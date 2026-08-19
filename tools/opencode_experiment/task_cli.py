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


SCHEMA = "telora.opencode-artifact-workflow/v1"
TASK_SCHEMA = "telora.oc-task-attempt/v1"
IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")


class TaskError(Exception):
    def __init__(self, message: str, code: int = 65):
        super().__init__(message)
        self.code = code


def _id(value: Any, where: str) -> str:
    if not isinstance(value, str) or not IDENTIFIER.fullmatch(value):
        raise TaskError(f"invalid {where}: {value!r}")
    return value


def _ids(value: Any, where: str) -> list[str]:
    if not isinstance(value, list):
        raise TaskError(f"{where} must be an id array")
    return [_id(item, where) for item in value]


def _paths(value: Any, where: str) -> list[str]:
    if not isinstance(value, list):
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


def _keys(value: dict[str, Any], allowed: set[str], where: str) -> None:
    unknown = set(value) - allowed
    if unknown:
        raise TaskError(f"unknown {where} key(s): {', '.join(sorted(unknown))}")


def _artifact_ref(value: Any, where: str) -> tuple[str, bool]:
    if not isinstance(value, str) or not value:
        raise TaskError(f"{where} must be an artifact id")
    optional = value.endswith("?")
    return _id(value[:-1] if optional else value, where), optional


def _artifact_owner(name: str, roles: list[str]) -> str | None:
    return next((role for role in roles if name.endswith(f".{role}")), None)


def validate_workflow(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TaskError("workflow must be an object")
    _keys(value, {"schema", "roles", "start_artifacts", "finish_artifact", "stop_path", "artifacts"},
          "workflow")
    if value.get("schema") != SCHEMA:
        raise TaskError("unsupported workflow schema")
    roles = _ids(value.get("roles", []), "workflow roles")
    if not roles or len(set(roles)) != len(roles):
        raise TaskError("workflow roles must be a nonempty unique id array")
    raw_artifacts = value.get("artifacts")
    if not isinstance(raw_artifacts, dict) or not raw_artifacts:
        raise TaskError("workflow artifacts must be a nonempty object")

    artifacts: dict[str, dict[str, Any]] = {}
    for raw_name, raw in raw_artifacts.items():
        name = _id(raw_name, "artifact id")
        if not isinstance(raw, dict):
            raise TaskError(f"artifact {name} must be an object")
        _keys(raw, {"id", "desc", "owner", "input", "checks", "instruction"}, f"artifact {name}")
        if raw.get("id", name) != name:
            raise TaskError(f"artifact id does not match its key: {name}")
        description = raw.get("desc")
        if not isinstance(description, str) or not description.strip():
            raise TaskError(f"artifact {name} desc must be nonempty")
        raw_inputs = raw.get("input", [])
        if not isinstance(raw_inputs, list):
            raise TaskError(f"artifact {name} input must be an artifact id array")
        inputs = []
        seen = set()
        for item in raw_inputs:
            if isinstance(item, dict):
                _keys(item, {"id", "optional"}, f"artifact {name} input")
                dependency = _id(item.get("id"), f"artifact {name} input")
                optional = item.get("optional")
                if not isinstance(optional, bool):
                    raise TaskError(f"artifact {name} input optional must be boolean")
            else:
                dependency, optional = _artifact_ref(item, f"artifact {name} input")
            if dependency in seen:
                raise TaskError(f"artifact {name} has duplicate input: {dependency}")
            seen.add(dependency)
            inputs.append({"id": dependency, "optional": optional})
        owner = _artifact_owner(name, roles)
        if raw.get("owner", owner) != owner:
            raise TaskError(f"artifact owner does not match its suffix: {name}")
        instruction = raw.get("instruction")
        if owner is not None and (not isinstance(instruction, str) or not instruction.strip()):
            raise TaskError(f"role-owned artifact {name} instruction must be nonempty")
        if owner is None and instruction is not None:
            raise TaskError(f"Host-owned artifact {name} cannot have an instruction")
        artifacts[name] = {
            "id": name,
            "desc": description,
            "owner": owner,
            "input": inputs,
            "checks": _paths(raw.get("checks", []), f"artifact {name} checks"),
            "instruction": instruction,
        }

    for artifact in artifacts.values():
        for dependency in artifact["input"]:
            if dependency["id"] not in artifacts:
                raise TaskError(f"artifact {artifact['id']} has unknown input: {dependency['id']}")

    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visiting:
            raise TaskError(f"artifact dependency cycle at: {name}")
        if name in visited:
            return
        visiting.add(name)
        for dependency in artifacts[name]["input"]:
            visit(dependency["id"])
        visiting.remove(name)
        visited.add(name)

    for name in artifacts:
        visit(name)

    start_artifacts = _ids(value.get("start_artifacts", []), "workflow start_artifacts")
    if not start_artifacts:
        raise TaskError("start_artifacts must not be empty")
    for name in start_artifacts:
        artifact = artifacts.get(name)
        if artifact is None or artifact["owner"] is not None or artifact["input"]:
            raise TaskError("start_artifacts must name root Host-owned artifacts")
    finish_artifact = _id(value.get("finish_artifact"), "finish artifact")
    if finish_artifact not in artifacts or artifacts[finish_artifact]["owner"] is not None:
        raise TaskError("finish_artifact must name a Host-owned artifact")
    stop_path = _paths([value.get("stop_path")], "workflow stop_path")[0]
    return {
        "schema": SCHEMA,
        "roles": roles,
        "start_artifacts": start_artifacts,
        "finish_artifact": finish_artifact,
        "stop_path": stop_path,
        "artifacts": artifacts,
    }


def load_workflow(root: Path) -> dict[str, Any]:
    try:
        manifest = json.loads((root / "experiment.json").read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise TaskError(f"missing experiment.json under {root}", 66) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise TaskError(f"invalid experiment.json: {exc}") from None
    return validate_workflow(manifest.get("workflow"))


def find_root(start: Path) -> Path:
    current = start.resolve()
    for candidate in (current, *current.parents):
        if (candidate / "experiment.json").is_file():
            return candidate
    raise TaskError("cannot find experiment.json from current directory", 66)


def _atomic_write(path: Path, content: bytes, minimum_ns: int = 0) -> int:
    previous = path.stat().st_mtime_ns if path.exists() else 0
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        stamp = max(time.time_ns(), previous + 1, minimum_ns + 1)
        os.utime(temporary, ns=(stamp, stamp))
        os.replace(temporary, path)
        return path.stat().st_mtime_ns
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextmanager
def _locked(root: Path) -> Iterator[None]:
    state = root / ".oc-task"
    state.mkdir(exist_ok=True)
    with (state / "lock").open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def _file_state(root: Path, patterns: list[str]) -> dict[str, Any]:
    paths = []
    missing = []
    for pattern in patterns:
        found = sorted(path for path in root.glob(pattern) if path.is_file() and not path.is_symlink())
        if not found:
            missing.append(pattern)
        paths.extend(found)
    paths = list(dict.fromkeys(paths))
    empty = [str(path.relative_to(root)) for path in paths if not path.stat().st_size]
    return {
        "ready": not missing and not empty,
        "files": [str(path.relative_to(root)) for path in paths],
        "missing": missing,
        "empty": empty,
    }


def _artifact_path(root: Path, name: str) -> Path:
    return root / "control" / "artifacts" / name


def _active_task_path(root: Path, role: str) -> Path:
    return root / ".oc-task" / "active" / f"{role}.json"


def _task_history_path(root: Path, task_id: str) -> Path:
    return root / ".oc-task" / "history" / f"{task_id}.json"


def _read_json(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, json.JSONDecodeError) as exc:
        raise TaskError(f"invalid task record {path}: {exc}") from None
    if not isinstance(value, dict):
        raise TaskError(f"invalid task record {path}")
    return value


def _write_json(path: Path, value: dict[str, Any]) -> None:
    _atomic_write(path, (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode())


def _task_response(workflow: dict[str, Any], status: dict[str, Any], task: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": "telora.oc-task-pull/v1",
        "role": task["role"],
        "task_id": task["task_id"],
        "started_at_ns": task["started_at_ns"],
        "artifacts": [{
            "id": name,
            "description": workflow["artifacts"][name]["desc"],
            "instruction": workflow["artifacts"][name]["instruction"],
            "inputs": [{**reference, "available": bool(
                status["artifacts"][reference["id"]]["stamp_mtime_ns"]
            )} for reference in workflow["artifacts"][name]["input"]],
            "checks": workflow["artifacts"][name]["checks"],
        } for name in task["artifacts"]],
    }


def task_records(root: Path) -> dict[str, list[dict[str, Any]]]:
    active = []
    history = []
    for path in sorted((root / ".oc-task" / "active").glob("*.json")):
        value = _read_json(path)
        if value is not None:
            active.append(value)
    for path in sorted((root / ".oc-task" / "history").glob("*.json")):
        value = _read_json(path)
        if value is not None:
            history.append(value)
    history.sort(key=lambda item: (item.get("started_at_ns", 0), item.get("task_id", "")))
    return {"active": active, "history": history}


def _task_inputs_current(status: dict[str, Any], task: dict[str, Any]) -> bool:
    inputs = task.get("inputs")
    artifacts = task.get("artifacts")
    return bool(isinstance(inputs, dict) and isinstance(artifacts, list) and all(
        name in status["artifacts"]
        and status["artifacts"][name]["input_mtime_ns"] == inputs.get(name)
        for name in artifacts
    ))


def _archive_active(root: Path, path: Path, task: dict[str, Any], status: str,
                    reason: str, ended_at_ns: int | None = None) -> dict[str, Any]:
    archived = dict(task)
    archived.update({
        "status": status,
        "ended_at_ns": time.time_ns() if ended_at_ns is None else ended_at_ns,
        "reason": reason,
    })
    _write_json(_task_history_path(root, task["task_id"]), archived)
    path.unlink(missing_ok=True)
    return archived


def evaluate(root: Path, workflow: dict[str, Any]) -> dict[str, Any]:
    artifacts = workflow["artifacts"]
    values: dict[str, dict[str, Any]] = {}

    def evaluate_one(name: str) -> dict[str, Any]:
        if name in values:
            return values[name]
        artifact = artifacts[name]
        dependencies = [(reference, evaluate_one(reference["id"])) for reference in artifact["input"]]
        checks = (_file_state(root, artifact["checks"]) if artifact["checks"] else
                  {"ready": True, "files": [], "missing": [], "empty": []})
        path = _artifact_path(root, name)
        stamp = path.stat().st_mtime_ns if path.is_file() else 0
        input_mtime = max((dependency["stamp_mtime_ns"] for reference, dependency in dependencies
                           if not reference["optional"] or dependency["stamp_mtime_ns"]), default=0)
        blocked_by = [reference["id"] for reference, dependency in dependencies
                      if (not reference["optional"] and not dependency["current"])
                      or (reference["optional"] and dependency["stamp_mtime_ns"]
                          and not dependency["current"])]
        ready = not blocked_by
        current = bool(stamp and checks["ready"] and ready and stamp > input_mtime)
        values[name] = {
            "id": name,
            "owner": artifact["owner"],
            "description": artifact["desc"],
            "current": current,
            "runnable": bool(artifact["owner"] and ready and not current),
            "publishable": bool(artifact["owner"] is None and ready and checks["ready"] and not current),
            "stamp_mtime_ns": stamp,
            "input_mtime_ns": input_mtime,
            "blocked_by": blocked_by,
            "checks": checks,
        }
        return values[name]

    for name in artifacts:
        evaluate_one(name)
    complete = values[workflow["finish_artifact"]]["current"]
    return {"schema": "telora.oc-artifact-status/v1", "artifacts": values,
            "complete": complete, "quiescent": complete}


def workflow_status(root: Path, workflow: dict[str, Any]) -> dict[str, Any]:
    with _locked(root):
        return evaluate(root, workflow)


def publish_artifact(root: Path, workflow: dict[str, Any], name: str) -> dict[str, Any]:
    artifact = workflow["artifacts"].get(name)
    if artifact is None:
        raise TaskError(f"unknown artifact: {name}", 64)
    if artifact["owner"] is not None:
        raise TaskError(f"role-owned artifact cannot be published by Host: {name}", 64)
    with _locked(root):
        value = evaluate(root, workflow)["artifacts"][name]
        if value["blocked_by"]:
            raise TaskError(f"artifact inputs are incomplete: {', '.join(value['blocked_by'])}", 75)
        if not value["checks"]["ready"]:
            raise TaskError(f"artifact checks are incomplete: {name}", 75)
        stamp = _atomic_write(_artifact_path(root, name), b"", value["input_mtime_ns"])
    return {"schema": "telora.oc-artifact/v1", "artifact": name, "mtime_ns": stamp}


def remove_artifact(root: Path, workflow: dict[str, Any], name: str) -> dict[str, Any]:
    artifact = workflow["artifacts"].get(name)
    if artifact is None:
        raise TaskError(f"unknown artifact: {name}", 64)
    if artifact["owner"] is not None:
        raise TaskError(f"role-owned artifact cannot be removed by Host: {name}", 64)
    with _locked(root):
        path = _artifact_path(root, name)
        existed = path.is_file()
        path.unlink(missing_ok=True)
    return {"schema": "telora.oc-artifact/v1", "artifact": name,
            "removed": True, "existed": existed}


def pull(root: Path, workflow: dict[str, Any], role: str,
         wait: bool, timeout: float | None) -> dict[str, Any]:
    if role not in workflow["roles"]:
        raise TaskError(f"unknown workflow role: {role}", 64)
    deadline = None if timeout is None else time.monotonic() + timeout
    while True:
        with _locked(root):
            if (root / workflow["stop_path"]).is_file():
                return {"schema": "telora.oc-task-stop/v1", "role": role, "stopped": True}
            status = evaluate(root, workflow)
            active_path = _active_task_path(root, role)
            active = _read_json(active_path)
            if active is not None:
                if _task_inputs_current(status, active):
                    return _task_response(workflow, status, active)
                _archive_active(root, active_path, active, "stale",
                                "artifact inputs changed after pull")
            runnable = [artifact for artifact in workflow["artifacts"].values()
                        if artifact["owner"] == role and status["artifacts"][artifact["id"]]["runnable"]]
            if runnable:
                started = time.time_ns()
                task = {
                    "schema": TASK_SCHEMA,
                    "task_id": f"{role}-{started}",
                    "role": role,
                    "artifacts": [artifact["id"] for artifact in runnable],
                    "inputs": {artifact["id"]: status["artifacts"][artifact["id"]]["input_mtime_ns"]
                               for artifact in runnable},
                    "started_at_ns": started,
                    "status": "active",
                }
                _write_json(active_path, task)
                return _task_response(workflow, status, task)
        if not wait or (deadline is not None and time.monotonic() >= deadline):
            status = workflow_status(root, workflow)
            waiting_for = [{
                "artifact": artifact["id"],
                "blocked_by": status["artifacts"][artifact["id"]]["blocked_by"],
            } for artifact in workflow["artifacts"].values()
                if artifact["owner"] == role
                and not status["artifacts"][artifact["id"]]["current"]]
            return {
                "schema": "telora.oc-task-wait/v1",
                "role": role,
                "waiting": True,
                "reason": ("waiting for artifact inputs" if waiting_for else
                           "all role artifacts are current; waiting for input changes or stop"),
                "waiting_for": waiting_for,
            }
        time.sleep(.2)


def submit(root: Path, workflow: dict[str, Any], role: str, names: list[str]) -> dict[str, Any]:
    if role not in workflow["roles"]:
        raise TaskError(f"unknown workflow role: {role}", 64)
    if not names or len(set(names)) != len(names):
        raise TaskError("submit requires unique artifact ids", 64)
    with _locked(root):
        active_path = _active_task_path(root, role)
        task = _read_json(active_path)
        if task is None:
            raise TaskError(f"role has no active pulled task: {role}", 75)
        expected = task.get("artifacts")
        if not isinstance(expected, list) or set(names) != set(expected) or len(names) != len(expected):
            raise TaskError(f"submit must contain the complete pulled task: {', '.join(expected or [])}", 64)
        status = evaluate(root, workflow)
        if not _task_inputs_current(status, task):
            _archive_active(root, active_path, task, "stale",
                            "artifact inputs changed after pull")
            raise TaskError("artifact inputs changed after pull", 75)
        values = []
        for name in names:
            artifact = workflow["artifacts"].get(name)
            if artifact is None:
                raise TaskError(f"unknown artifact: {name}", 64)
            if artifact["owner"] != role:
                raise TaskError(f"artifact is not owned by {role}: {name}", 64)
            value = status["artifacts"][name]
            if not value["runnable"]:
                raise TaskError(f"artifact is not runnable: {name}", 75)
            if not value["checks"]["ready"]:
                raise TaskError(f"artifact checks are incomplete: {name}", 75)
            values.append((name, value))
        published = [{"artifact": name, "mtime_ns": _atomic_write(
            _artifact_path(root, name), b"", value["input_mtime_ns"]
        )} for name, value in values]
        completed = dict(task)
        completed.update({"status": "submitted", "submitted_at_ns": time.time_ns(),
                          "artifacts_published": published})
        _write_json(_task_history_path(root, task["task_id"]), completed)
        active_path.unlink(missing_ok=True)
    return {"schema": "telora.oc-task-submit/v1", "role": role,
            "task_id": task["task_id"], "artifacts": published}


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-task", description="Run an artifact touch-file DAG.")
    value.add_argument("--root", type=Path)
    commands = value.add_subparsers(dest="command", required=True)
    pull_command = commands.add_parser("pull")
    pull_command.add_argument("role")
    pull_command.add_argument("--no-wait", action="store_true")
    pull_command.add_argument("--timeout", type=float, default=60.0)
    submit_command = commands.add_parser("submit")
    submit_command.add_argument("role")
    submit_command.add_argument("artifacts", nargs="+")
    commands.add_parser("status")
    return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        root = args.root.resolve() if args.root else find_root(Path.cwd())
        workflow = load_workflow(root)
        if args.command == "pull":
            result = pull(root, workflow, _id(args.role, "role"), not args.no_wait, args.timeout)
        elif args.command == "submit":
            result = submit(root, workflow, _id(args.role, "role"),
                            [_id(name, "artifact") for name in args.artifacts])
        else:
            result = workflow_status(root, workflow)
        print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
        return 0
    except TaskError as exc:
        print(f"oc-task: {exc}", file=sys.stderr)
        return exc.code
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
