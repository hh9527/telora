#!/usr/bin/env python3
from __future__ import annotations

import argparse
import fcntl
import json
import os
import re
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path, PurePosixPath
from typing import Any, Iterator


SCHEMA = "telora.opencode-node-workflow/v1"
IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")
NODE_ID = re.compile(r"[a-z0-9][a-z0-9._/-]*\Z")


class TaskError(Exception):
    def __init__(self, message: str, code: int = 65):
        super().__init__(message); self.code = code


def _id(value: Any, where: str, node: bool = False) -> str:
    pattern = NODE_ID if node else IDENTIFIER
    if not isinstance(value, str) or not pattern.fullmatch(value) or ".." in value.split("/"):
        raise TaskError(f"invalid {where}: {value!r}")
    return value


def _paths(value: Any, where: str, allow_empty: bool = False) -> list[str]:
    if not isinstance(value, list) or (not value and not allow_empty):
        raise TaskError(f"{where} must be a path array")
    result = []
    for item in value:
        if not isinstance(item, str) or not item:
            raise TaskError(f"{where} must be a path array")
        path = PurePosixPath(item)
        if path.is_absolute() or any(part in ("", ".", "..") for part in item.split("/")):
            raise TaskError(f"unsafe workflow path: {item!r}")
        result.append(item)
    return result


def _ids(value: Any, where: str, node: bool = False) -> list[str]:
    if not isinstance(value, list):
        raise TaskError(f"{where} must be an id array")
    return [_id(item, where, node) for item in value]


def _keys(value: dict[str, Any], allowed: set[str], where: str) -> None:
    unknown = set(value) - allowed
    if unknown: raise TaskError(f"unknown {where} key(s): {', '.join(sorted(unknown))}")


def _node_kind(node_id: str) -> str:
    if node_id.endswith(".rc"): return "rc"
    if node_id.endswith(".ready"): return "ready"
    if node_id.endswith(".feedback"): return "feedback"
    raise TaskError(f"node id must end in .rc, .ready, or .feedback: {node_id}")


def validate_workflow(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict): raise TaskError("workflow must be an object")
    _keys(value, {"schema", "start_nodes", "finish_node", "stop_path", "nodes", "tasks"}, "workflow")
    if value.get("schema") != SCHEMA: raise TaskError("unsupported workflow schema")
    raw_nodes, raw_tasks = value.get("nodes"), value.get("tasks")
    if not isinstance(raw_nodes, list) or not raw_nodes or not isinstance(raw_tasks, list) or not raw_tasks:
        raise TaskError("workflow nodes and tasks must be nonempty arrays")

    nodes = []
    known_nodes: set[str] = set()
    for raw in raw_nodes:
        if not isinstance(raw, dict): raise TaskError("workflow node must be an object")
        _keys(raw, {"id", "kind", "role", "needs", "inputs", "checks", "observes"}, "workflow node")
        node_id = _id(raw.get("id"), "node id", True); kind = _node_kind(node_id)
        if raw.get("kind", kind) != kind: raise TaskError(f"node kind does not match its suffix: {node_id}")
        if node_id in known_nodes: raise TaskError(f"duplicate workflow node: {node_id}")
        known_nodes.add(node_id)
        role = raw.get("role")
        if kind == "rc": role = _id(role, f"node {node_id} role")
        elif role is not None: raise TaskError(f"Host-owned node {node_id} cannot have a role")
        nodes.append({"id": node_id, "kind": kind, "role": role,
                      "needs": _ids(raw.get("needs", []), f"node {node_id} needs", True),
                      "inputs": _ids(raw.get("inputs", []), f"node {node_id} inputs", True),
                      "checks": _paths(raw.get("checks", []), f"node {node_id} checks", True),
                      "observes": raw.get("observes")})

    tasks = []
    known_tasks: set[str] = set()
    for raw in raw_tasks:
        if not isinstance(raw, dict): raise TaskError("workflow task must be an object")
        _keys(raw, {"id", "role", "needs", "after", "absorbs", "inputs", "outputs", "instruction"}, "workflow task")
        task_id = _id(raw.get("id"), "task id"); role = _id(raw.get("role"), "task role")
        if not task_id.endswith(".rc"): raise TaskError(f"workflow task id must end in .rc: {task_id}")
        if task_id in known_tasks: raise TaskError(f"duplicate workflow task: {task_id}")
        known_tasks.add(task_id)
        instruction = raw.get("instruction")
        if not isinstance(instruction, str) or not instruction.strip(): raise TaskError(f"task {task_id} instruction must be nonempty")
        tasks.append({"id": task_id, "role": role,
                      "needs": _ids(raw.get("needs", []), f"task {task_id} needs", True),
                      "after": _ids(raw.get("after", []), f"task {task_id} after"),
                      "absorbs": _ids(raw.get("absorbs", []), f"task {task_id} absorbs"),
                      "inputs": _paths(raw.get("inputs", []), f"task {task_id} inputs", True),
                      "outputs": _paths(raw.get("outputs", []), f"task {task_id} outputs", True),
                      "instruction": instruction})

    by_node = {item["id"]: item for item in nodes}; by_task = {item["id"]: item for item in tasks}
    for node in nodes:
        for dependency in (*node["needs"], *node["inputs"]):
            if dependency not in by_node: raise TaskError(f"node {node['id']} has unknown dependency: {dependency}")
        if any(by_node[item]["kind"] != "feedback" for item in node["inputs"]):
            raise TaskError(f"node {node['id']} inputs must name optional .feedback nodes")
        if node["observes"] is not None:
            node["observes"] = _id(node["observes"], f"node {node['id']} observes", True)
            if (node["kind"] != "feedback" or node["observes"] not in by_node
                    or by_node[node["observes"]]["kind"] != "rc"):
                raise TaskError(f"invalid observed node for {node['id']}")
        elif node["kind"] == "feedback":
            raise TaskError(f"feedback node must observe an .rc node: {node['id']}")
    producers = set()
    for task in tasks:
        for dependency in task["needs"]:
            if dependency not in by_node: raise TaskError(f"task {task['id']} needs unknown node: {dependency}")
        for dependency in task["after"]:
            if dependency not in by_task: raise TaskError(f"task {task['id']} follows unknown task: {dependency}")
        node = by_node.get(task["id"])
        if not node or node["kind"] != "rc" or node["role"] != task["role"]:
            raise TaskError(f"task {task['id']} must match an .rc node owned by its role")
        producers.add(task["id"])
        for absorbed in task["absorbs"]:
            target = by_task.get(absorbed)
            if not target or target["role"] != task["role"] or absorbed == task["id"]:
                raise TaskError(f"task {task['id']} can only absorb another task owned by {task['role']}")
    for node in nodes:
        if node["kind"] == "rc" and node["id"] not in producers: raise TaskError(f"rc node has no matching task: {node['id']}")

    visiting: set[str] = set(); visited: set[str] = set()
    def visit(node_id: str) -> None:
        if node_id in visiting: raise TaskError(f"workflow node dependency cycle at: {node_id}")
        if node_id in visited: return
        visiting.add(node_id)
        for dependency in by_node[node_id]["needs"]: visit(dependency)
        visiting.remove(node_id); visited.add(node_id)
    for node_id in by_node: visit(node_id)

    visiting_tasks: set[str] = set(); visited_tasks: set[str] = set()
    def visit_task(task_id: str) -> None:
        if task_id in visiting_tasks: raise TaskError(f"workflow task absorption cycle at: {task_id}")
        if task_id in visited_tasks: return
        visiting_tasks.add(task_id)
        for absorbed in by_task[task_id]["absorbs"]: visit_task(absorbed)
        visiting_tasks.remove(task_id); visited_tasks.add(task_id)
    for task_id in by_task: visit_task(task_id)

    start_nodes = _ids(value.get("start_nodes", []), "workflow start_nodes", True)
    if not start_nodes or any(item not in by_node or by_node[item]["kind"] != "ready" for item in start_nodes):
        raise TaskError("start_nodes must name Host-owned .ready nodes")
    finish_node = _id(value.get("finish_node"), "finish node", True)
    if finish_node not in by_node or by_node[finish_node]["kind"] != "ready": raise TaskError("finish_node must name a .ready node")
    stop_path = _paths([value.get("stop_path")], "workflow stop_path")[0]
    return {"schema": SCHEMA, "start_nodes": start_nodes, "finish_node": finish_node,
            "stop_path": stop_path, "nodes": nodes, "tasks": tasks}


def load_workflow(root: Path) -> dict[str, Any]:
    try: manifest = json.loads((root / "experiment.json").read_text(encoding="utf-8"))
    except FileNotFoundError: raise TaskError(f"missing experiment.json under {root}", 66) from None
    except (OSError, json.JSONDecodeError) as exc: raise TaskError(f"invalid experiment.json: {exc}") from None
    return validate_workflow(manifest.get("workflow"))


def find_root(start: Path) -> Path:
    current = start.resolve()
    for candidate in (current, *current.parents):
        if (candidate / "experiment.json").is_file(): return candidate
    raise TaskError("cannot find experiment.json from current directory", 66)


def _atomic_write(path: Path, content: bytes, minimum_ns: int = 0) -> int:
    previous = path.stat().st_mtime_ns if path.exists() else 0; path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output: output.write(content); output.flush(); os.fsync(output.fileno())
        stamp = max(time.time_ns(), previous + 1, minimum_ns + 1); os.utime(temporary, ns=(stamp, stamp)); os.replace(temporary, path)
        return path.stat().st_mtime_ns
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def _atomic_json(path: Path, value: Any) -> int:
    return _atomic_write(path, (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode())


@contextmanager
def _locked(root: Path) -> Iterator[None]:
    state = root / ".oc-task"; state.mkdir(exist_ok=True)
    with (state / "lock").open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try: yield
        finally: fcntl.flock(lock, fcntl.LOCK_UN)


def _matches(root: Path, pattern: str) -> list[Path]:
    return sorted(path for path in root.glob(pattern) if path.is_file() and not path.is_symlink())


def _file_state(root: Path, patterns: list[str], nonempty: bool = True) -> dict[str, Any]:
    paths = []; missing = []
    for pattern in patterns:
        found = _matches(root, pattern)
        if not found: missing.append(pattern)
        paths.extend(found)
    paths = list(dict.fromkeys(paths)); empty = [str(path.relative_to(root)) for path in paths if nonempty and not path.stat().st_size]
    return {"ready": not missing and not empty, "mtime_ns": max((p.stat().st_mtime_ns for p in paths), default=0),
            "files": [str(p.relative_to(root)) for p in paths], "missing": missing, "empty": empty}


def _read_json(path: Path) -> dict[str, Any] | None:
    try: value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError: return None
    except (OSError, json.JSONDecodeError) as exc: raise TaskError(f"invalid task state {path}: {exc}") from None
    if not isinstance(value, dict): raise TaskError(f"invalid task state {path}")
    return value


def _node_path(root: Path, node_id: str) -> Path: return root / "control" / "nodes" / node_id
def _done_path(root: Path, task_id: str) -> Path: return root / ".oc-task" / "done" / f"{task_id}.done"
def _claim_path(root: Path, role: str) -> Path: return root / ".oc-task" / "claims" / f"{role}.json"


def evaluate(root: Path, workflow: dict[str, Any]) -> dict[str, Any]:
    nodes = {item["id"]: item for item in workflow["nodes"]}; tasks = {item["id"]: item for item in workflow["tasks"]}
    node_values: dict[str, dict[str, Any]] = {}; task_values: dict[str, dict[str, Any]] = {}
    pending_nodes = list(nodes.values()); pending_tasks = list(tasks.values())
    while pending_nodes or pending_tasks:
        progressed = False
        for node in list(pending_nodes):
            if any(item not in node_values for item in node["needs"]): continue
            checks = _file_state(root, node["checks"]) if node["checks"] else {"ready": True, "mtime_ns": 0, "files": [], "missing": [], "empty": []}
            path = _node_path(root, node["id"]); stamp = path.stat().st_mtime_ns if path.is_file() else 0
            dependencies = [node_values[item] for item in node["needs"]]
            input_mtimes = []
            for item in node["inputs"]:
                input_path = _node_path(root, item)
                input_mtimes.append(input_path.stat().st_mtime_ns if input_path.is_file() else 0)
            prerequisites_ready = all(node_values[item]["current"] for item in node["needs"])
            observed_mtime = 0
            if node["observes"] is not None:
                observed_path = _node_path(root, node["observes"])
                observed_mtime = observed_path.stat().st_mtime_ns if observed_path.is_file() else 0
            prerequisite_mtime = max([checks["mtime_ns"], observed_mtime,
                                      *(x["mtime_ns"] for x in dependencies),
                                      *input_mtimes], default=0)
            current = bool(stamp and checks["ready"] and prerequisites_ready and stamp > prerequisite_mtime)
            node_values[node["id"]] = {"id": node["id"], "kind": node["kind"], "role": node["role"],
                "current": current, "mtime_ns": stamp if current else prerequisite_mtime, "stamp_mtime_ns": stamp,
                "prerequisite_mtime_ns": prerequisite_mtime, "checks": checks}
            pending_nodes.remove(node); progressed = True
        for task in list(pending_tasks):
            if any(item not in node_values for item in task["needs"]) or any(item not in task_values for item in task["after"]): continue
            dependencies = [node_values[item] for item in task["needs"]]; preceding = [task_values[item] for item in task["after"]]
            inputs = _file_state(root, task["inputs"], False) if task["inputs"] else {"ready": True, "mtime_ns": 0, "files": [], "missing": [], "empty": []}
            outputs = _file_state(root, task["outputs"]) if task["outputs"] else {"ready": True, "mtime_ns": 0, "files": [], "missing": [], "empty": []}
            ready = inputs["ready"] and all(item["current"] for item in (*dependencies, *preceding))
            prerequisite_mtime = max([inputs["mtime_ns"], *(x["mtime_ns"] for x in dependencies), *(x["mtime_ns"] for x in preceding)], default=0)
            done = _done_path(root, task["id"]); done_stamp = done.stat().st_mtime_ns if done.is_file() else 0
            produced = node_values[task["id"]]
            current = bool(ready and outputs["ready"] and done_stamp > max(prerequisite_mtime, outputs["mtime_ns"])
                           and produced["current"])
            task_values[task["id"]] = {"id": task["id"], "kind": "task", "role": task["role"], "ready": ready,
                "current": current, "runnable": ready and not current, "mtime_ns": done_stamp if current else prerequisite_mtime,
                "done_mtime_ns": done_stamp, "prerequisite_mtime_ns": prerequisite_mtime, "inputs": inputs, "outputs": outputs}
            pending_tasks.remove(task); progressed = True
        if not progressed: raise TaskError("workflow dependencies could not be evaluated")
    claims = {role: claim.get("task") for role in sorted({t["role"] for t in tasks.values()}) if (claim := _read_json(_claim_path(root, role)))}
    finish = node_values[workflow["finish_node"]]["current"]
    return {"schema": "telora.oc-task-status/v2", "nodes": node_values, "tasks": task_values,
            "claims": claims, "complete": finish, "quiescent": finish and not claims}


def workflow_status(root: Path, workflow: dict[str, Any]) -> dict[str, Any]:
    with _locked(root): return evaluate(root, workflow)


def publish_node(root: Path, workflow: dict[str, Any], node_id: str, content: bytes | None = None) -> dict[str, Any]:
    nodes = {item["id"]: item for item in workflow["nodes"]}; node = nodes.get(node_id)
    if not node or node["kind"] == "rc": raise TaskError(f"not a Host-owned workflow node: {node_id}", 64)
    with _locked(root):
        status = evaluate(root, workflow); current = status["nodes"][node_id]
        if node["kind"] == "feedback":
            observed = status["nodes"][node["observes"]]
            if not observed["current"]: raise TaskError(f"cannot publish feedback for stale node: {node['observes']}", 75)
            if content is None or not content.strip(): raise TaskError("feedback content must not be empty", 64)
        elif content is not None: raise TaskError("ready nodes do not accept content", 64)
        if not current["checks"]["ready"] or any(not status["nodes"][x]["current"] for x in node["needs"]):
            raise TaskError(f"node prerequisites or checks are incomplete: {node_id}", 75)
        stamp = _atomic_write(_node_path(root, node_id), content or b"", current["prerequisite_mtime_ns"])
    return {"schema": "telora.oc-node/v1", "node": node_id, "mtime_ns": stamp}


def _claim_result(task: dict[str, Any], claim: dict[str, Any], resumed: bool,
                  status: dict[str, Any]) -> dict[str, Any]:
    absorbed = []
    completed = set(claim.get("absorbed_done", []))
    for task_id, input_mtime_ns in claim.get("absorbed", {}).items():
        target = status["tasks"].get(task_id)
        if target is None: raise TaskError(f"invalid absorbed task in claim: {task_id}")
        if task_id in completed and target["current"]: continue
        instruction = claim.get("absorbed_instructions", {}).get(task_id, "")
        absorbed.append({"task": task_id, "instruction": instruction,
                         "inputs_changed": target["prerequisite_mtime_ns"] > int(input_mtime_ns)})
    primary = status["tasks"][task["id"]]
    return {"schema": "telora.oc-task-claim/v3", "role": task["role"], "task": task["id"],
            "instruction": task["instruction"], "absorbed": absorbed, "resumed": resumed,
            "inputs_changed": primary["prerequisite_mtime_ns"] > int(claim["input_mtime_ns"])
                              or any(item["inputs_changed"] for item in absorbed)}


def next_task(root: Path, workflow: dict[str, Any], role: str, wait: bool, timeout: float | None) -> dict[str, Any]:
    tasks = workflow["tasks"]
    if role not in {task["role"] for task in tasks}: raise TaskError(f"unknown workflow role: {role}", 64)
    deadline = None if timeout is None else time.monotonic() + timeout
    while True:
        with _locked(root):
            if (root / workflow["stop_path"]).is_file(): return {"schema": "telora.oc-task-stop/v1", "role": role, "stopped": True}
            status = evaluate(root, workflow); claim_path = _claim_path(root, role); claim = _read_json(claim_path)
            if claim:
                task = next((x for x in tasks if x["id"] == claim.get("task") and x["role"] == role), None)
                if not task: raise TaskError(f"invalid active claim for role {role}")
                return _claim_result(task, claim, True, status)
            runnable = [x for x in tasks if x["role"] == role and status["tasks"][x["id"]]["runnable"]]
            runnable_ids = {item["id"] for item in runnable}
            task = next((x for x in runnable if any(item in runnable_ids for item in x["absorbs"])), None)
            if task is None: task = next(iter(runnable), None)
            if task:
                value = status["tasks"][task["id"]]
                absorbed_tasks = [x for x in task["absorbs"] if x in runnable_ids]
                by_id = {item["id"]: item for item in tasks}
                claim = {"task": task["id"], "input_mtime_ns": value["prerequisite_mtime_ns"],
                         "absorbed": {item: status["tasks"][item]["prerequisite_mtime_ns"] for item in absorbed_tasks},
                         "absorbed_done": [],
                         "absorbed_instructions": {item: by_id[item]["instruction"] for item in absorbed_tasks}}
                _atomic_json(claim_path, claim)
                return _claim_result(task, claim, False, status)
        if not wait or (deadline is not None and time.monotonic() >= deadline): return {"schema": "telora.oc-task-wait/v1", "role": role, "waiting": True}
        time.sleep(.25)


def mark_done(root: Path, workflow: dict[str, Any], role: str, task_id: str) -> dict[str, Any]:
    if not task_id.endswith(".rc"): raise TaskError("mark-done target must end in .rc", 64)
    tasks = {item["id"]: item for item in workflow["tasks"]}; task = tasks.get(task_id)
    if not task or task["role"] != role: raise TaskError(f"task {task_id!r} does not belong to role {role!r}", 64)
    with _locked(root):
        status = evaluate(root, workflow); value = status["tasks"][task_id]; claim_path = _claim_path(root, role); claim = _read_json(claim_path)
        if not claim:
            if value["current"]: return {"schema": "telora.oc-task-done/v3", "role": role, "task": task_id,
                                         "node": task_id, "claim_retained": False, "idempotent": True}
            raise TaskError(f"role {role} has no active claim", 75)
        primary_id = claim.get("task")
        primary = tasks.get(primary_id)
        if not primary or primary["role"] != role: raise TaskError(f"invalid active claim for role {role}")
        absorbed = dict(claim.get("absorbed", {}))
        absorbed_done = set(claim.get("absorbed_done", []))
        if task_id != primary_id and task_id not in absorbed:
            if task_id in primary["absorbs"] and value["current"]:
                return {"schema": "telora.oc-task-done/v3", "role": role, "task": task_id,
                        "parent": primary_id, "claim_retained": True, "idempotent": True}
            raise TaskError(f"role {role} is currently working on {primary_id}", 75)
        snapshot = claim["input_mtime_ns"] if task_id == primary_id else absorbed[task_id]
        if task_id != primary_id and task_id in absorbed_done and value["current"]:
            return {"schema": "telora.oc-task-done/v3", "role": role, "task": task_id,
                    "parent": primary_id, "claim_retained": True, "idempotent": True}
        if value["prerequisite_mtime_ns"] > int(snapshot):
            claim_path.unlink(); raise TaskError("task inputs changed while it was running; claim released", 75)
        if task_id == primary_id:
            incomplete = [item for item in absorbed
                          if item not in absorbed_done or not status["tasks"][item]["current"]]
            if incomplete: raise TaskError(f"absorbed tasks must be completed first: {', '.join(incomplete)}", 75)
        if not value["ready"] or not value["outputs"]["ready"]: raise TaskError("task prerequisites or outputs are incomplete", 75)
        minimum = max(value["prerequisite_mtime_ns"], value["outputs"]["mtime_ns"])
        node = next(x for x in workflow["nodes"] if x["id"] == task_id)
        checks = _file_state(root, node["checks"]) if node["checks"] else {"ready": True, "mtime_ns": 0}
        if not checks["ready"]: raise TaskError(f"node output checks are incomplete: {task_id}", 75)
        done_stamp = _atomic_write(_done_path(root, task_id), b"", minimum)
        node_stamp = _atomic_write(_node_path(root, task_id), b"", max(done_stamp, checks["mtime_ns"]))
        retained = task_id != primary_id
        if retained:
            absorbed_done.add(task_id)
            claim["absorbed_done"] = sorted(absorbed_done)
            _atomic_json(claim_path, claim)
        else:
            claim_path.unlink()
    return {"schema": "telora.oc-task-done/v3", "role": role, "task": task_id, "node": task_id,
            "parent": primary_id if retained else None, "claim_retained": retained,
            "mtime_ns": node_stamp, "idempotent": False}


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-task", description="Run recipes for a file-driven node DAG."); value.add_argument("--root", type=Path)
    commands = value.add_subparsers(dest="command", required=True)
    nxt = commands.add_parser("next"); nxt.add_argument("role"); nxt.add_argument("--no-wait", action="store_true"); nxt.add_argument("--timeout", type=float)
    done = commands.add_parser("mark-done"); done.add_argument("role"); done.add_argument("task")
    commands.add_parser("status"); return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        root = args.root.resolve() if args.root else find_root(Path.cwd()); workflow = load_workflow(root)
        if args.command == "next": result = next_task(root, workflow, _id(args.role, "role"), not args.no_wait, args.timeout)
        elif args.command == "mark-done": result = mark_done(root, workflow, _id(args.role, "role"), _id(args.task, "task"))
        else: result = workflow_status(root, workflow)
        print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True)); return 75 if result.get("waiting") else 0
    except TaskError as exc: print(f"oc-task: {exc}", file=sys.stderr); return exc.code
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
