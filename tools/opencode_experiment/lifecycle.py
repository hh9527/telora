from __future__ import annotations

import hashlib
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any

from .client import Client
from .config import ControlError, Manifest, repository_root, sha256, validate_identifier, load_manifest
from .context import Context
from .external import resolve_cli, resolve_command
from .observe import latest_assistant, normalized, text_parts
from .state import SCHEMA, atomic_json, atomic_write, bind_plan, load_state, locked, now, save_state


def git_metadata(repo: Path) -> tuple[str, bool]:
    git = resolve_cli("git")
    revision = subprocess.run([*git, "rev-parse", "HEAD"], cwd=repo, text=True, check=True, stdout=subprocess.PIPE).stdout.strip()
    dirty = bool(subprocess.run([*git, "status", "--porcelain"], cwd=repo, text=True, check=True, stdout=subprocess.PIPE).stdout)
    return revision, dirty


def _copy_file(source: Path, destination: Path, mode: int) -> None:
    if not source.is_file() or source.is_symlink(): raise ControlError(f"missing or unsafe input: {source}", 66)
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination); destination.chmod(mode)


def _copy_template(source: Path, destination: Path) -> None:
    if not source.is_dir() or source.is_symlink(): raise ControlError(f"missing workspace template: {source}", 66)
    for path in source.rglob("*"):
        if path.is_symlink(): raise ControlError(f"workspace template contains a symlink: {path}")
    shutil.copytree(source, destination, dirs_exist_ok=True, symlinks=False)


def _opencode_config(manifest: Manifest) -> dict[str, Any]:
    reads = manifest.permissions["read"]
    writes = manifest.permissions["write"]
    read_rules: dict[str, str] = {"*": "deny"}
    for value in reads:
        read_rules[value] = "allow"; read_rules[f"**/{value}"] = "allow"
    list_rules = dict(read_rules)
    for value in reads:
        base = value.removesuffix("/**")
        list_rules[base] = "allow"; list_rules[f"**/{base}"] = "allow"
    write_rules: dict[str, str] = {"*": "deny"}
    for value in writes:
        write_rules[value] = "allow"; write_rules[f"**/{value}"] = "allow"
    feedback = manifest.feedback["path"]
    write_rules[feedback] = "deny"; write_rules[f"**/{feedback}"] = "deny"
    bash = {"*": "deny", **{command: "allow" for command in manifest.permissions["commands"]}, "__no_more_commands__": "deny"}
    return {"$schema": "https://opencode.ai/config.json", "permission": {
        "read": read_rules, "list": list_rules, "glob": read_rules, "grep": read_rules,
        "edit": write_rules, "write": write_rules, "bash": bash, "task": "deny",
        "webfetch": "deny", "external_directory": "deny",
    }}


def _port_free(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(.2)
        return sock.connect_ex(("127.0.0.1", port)) != 0


def verify_prepared(manifest: Manifest, state: dict[str, Any]) -> None:
    workspace = Path(state["workspace"])
    for item in manifest.copies:
        name = str(item["from"]); source = manifest.source(name); destination = workspace / str(item["to"])
        expected = state["input_hashes"].get(name)
        if not source.is_file() or sha256(source) != expected:
            raise ControlError(f"plan input changed since preparation: {name}")
        if not destination.is_file() or destination.is_symlink() or sha256(destination) != expected:
            raise ControlError(f"workspace input changed since preparation: {item['to']}")
    if sha256(manifest.root / "experiment.json") != state["input_hashes"].get("experiment.json"):
        raise ControlError("experiment.json changed since preparation")
    for item in manifest.artifacts:
        name = str(item["name"]); destination = workspace / str(item["to"])
        if not destination.is_file() or destination.is_symlink() or sha256(destination) != state["binary_hashes"].get(name):
            raise ControlError(f"workspace artifact changed since preparation: {name}")


def prepare(plan_id: str, exec_name: str, port: int | None, artifacts: dict[str, str] | None = None) -> tuple[Path, dict[str, Any], bool]:
    repo = repository_root(); validate_identifier(plan_id, "plan-id"); validate_identifier(exec_name, "exec-name")
    if port is not None and (port < 1 or port > 65535): raise ControlError("port must be from 1 through 65535", 64)
    legacy = repo / "target" / "exp" / "dir"
    if legacy.exists():
        raise ControlError(f"legacy temporary controller state exists at {legacy}; remove it before using named executions")
    manifest = load_manifest(repo, plan_id); root = bind_plan(repo, plan_id, exec_name)
    with locked(root):
        if (root / "state.json").exists():
            state = load_state(root)
            if state["plan_id"] != plan_id: raise ControlError("execution plan mismatch")
            if state["phase"] in ("finished", "retired", "failed"): raise ControlError(f"execution {exec_name} is {state['phase']}")
            if port is not None and int(state["server_url"].rsplit(":", 1)[1]) != port: raise ControlError(f"execution already uses port {state['server_url'].rsplit(':',1)[1]}", 64)
            if not Path(state["workspace"]).is_dir(): raise ControlError("recorded workspace is missing", 66)
            verify_prepared(manifest, state)
            return root, state, False
        port = port or 4096
        revision, dirty = git_metadata(repo)
        run_root = Path(tempfile.mkdtemp(prefix=f"oc-exp-{exec_name}-", dir="/tmp")).resolve(); workspace = run_root / "ws"
        state: dict[str, Any] = {"schema": SCHEMA, "plan_id": plan_id, "exec_name": exec_name, "run_id": uuid.uuid4().hex,
            "phase": "preparing", "workspace": str(workspace), "run_root": str(run_root), "session_id": None,
            "server_url": f"http://127.0.0.1:{port}", "repository_revision": revision, "repository_dirty": dirty,
            "input_hashes": {}, "binary_hashes": {}, "next_round": 0, "active_round": None,
            "created_at": now(), "started_at": None, "finished_at": None}
        save_state(root, state)
        try:
            workspace.mkdir()
            _copy_template(manifest.source(manifest.template), workspace)
            feedback_path = workspace / str(manifest.feedback["path"])
            if not feedback_path.exists():
                atomic_write(feedback_path, b"")
            for item in manifest.copies:
                source = manifest.source(str(item["from"])); target = workspace / str(item["to"])
                _copy_file(source, target, int(str(item["mode"]), 8)); state["input_hashes"][str(item["from"])] = sha256(source)
            artifact_overrides = artifacts or {}
            for item in manifest.artifacts:
                name = str(item["name"]); source_value = artifact_overrides.get(name, str(item["source"])); source = Path(source_value)
                if not source.is_absolute(): source = repo / source
                if not source.is_file() and item.get("build"):
                    result = subprocess.run(resolve_command(item["build"], repo), cwd=repo)
                    if result.returncode: raise ControlError(f"artifact build failed: {name}", 70)
                target = workspace / str(item["to"]); _copy_file(source.resolve(), target, int(str(item.get("mode", "0555")), 8)); state["binary_hashes"][name] = sha256(target)
            atomic_json(workspace / "opencode.json", _opencode_config(manifest))
            state["input_hashes"]["experiment.json"] = sha256(manifest.root / "experiment.json")
            save_state(root, state)
        except Exception:
            state["phase"] = "failed"; save_state(root, state); raise
    return root, state, True


def create_empty_session(root: Path, state: dict[str, Any], title: str) -> dict[str, Any]:
    if state.get("session_id"): return state
    port = int(state["server_url"].rsplit(":", 1)[1])
    if not _port_free(port): raise ControlError(f"port {port} is occupied", 69)
    log = (root / "handshake.log").open("ab")
    process = subprocess.Popen([*resolve_cli("opencode"), "serve", "--hostname", "127.0.0.1", "--port", str(port), "--pure"], cwd=state["workspace"], stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    client = Client(state["server_url"], state["workspace"])
    try:
        healthy = False
        for _ in range(100):
            if process.poll() is not None: break
            try: client.health(); healthy = True; break
            except ControlError: time.sleep(.1)
        if not healthy: raise ControlError(f"temporary opencode daemon did not become healthy; see {root / 'handshake.log'}", 70)
        response = client.create_session(title); session_id = response.get("id") if isinstance(response, dict) else None
        if not isinstance(session_id, str) or not session_id.startswith("ses_"): raise ControlError("opencode returned an invalid session identity")
        with locked(root):
            current = load_state(root); current["session_id"] = session_id; current["phase"] = "ready"; save_state(root, current); state = current
    finally:
        process.terminate()
        try: process.wait(timeout=5)
        except subprocess.TimeoutExpired: process.kill(); process.wait()
        log.close()
    return state


def live_boundary(context: Context, allow_length: bool = False) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    client = context.client(); client.health(); status = client.status()
    if status.get("type", "idle") != "idle": raise ControlError(f"session is {status.get('type')}; operation requires idle", 75)
    messages = client.messages(); latest = latest_assistant(messages)
    if not latest: raise ControlError("session has no assistant completion")
    info = latest.get("info", {}); finish = info.get("finish")
    if info.get("time", {}).get("completed") is None or finish not in (("stop", "length") if allow_length else ("stop",)):
        raise ControlError(f"latest assistant message is not a completed {'stop/length' if allow_length else 'stop'}")
    return messages, latest


def _remote_user_with_text(messages: list[dict[str, Any]], text: str, after_message_id: str | None = None) -> dict[str, Any] | None:
    if after_message_id:
        indexes = [i for i, message in enumerate(messages) if message.get("info", {}).get("id") == after_message_id]
        messages = messages[indexes[-1] + 1:] if indexes else messages
    matches = [m for m in messages if m.get("info", {}).get("role") == "user" and text_parts(m) == [text]]
    return matches[-1] if matches else None


def send_round(context: Context, kind: str, text: str, *, require_empty: bool = False, require_finish: str = "stop", source: dict[str, Any] | None = None) -> dict[str, Any]:
    digest = hashlib.sha256(text.encode()).hexdigest()
    # Observation is part of every mutation; callers do not need a separate
    # status command to close the preceding round locally.
    if context.state.get("active_round"):
        context.state, _ = reconcile(context)
    with locked(context.root):
        state = load_state(context.root); client = context.client(); client.health(); status = client.status()
        if status.get("type", "idle") != "idle": raise ControlError(f"session is {status.get('type')}; refusing to send", 75)
        messages = client.messages()
        active = state.get("active_round")
        if active:
            record_path = context.root / "rounds" / active["file"]
            record = json.loads(record_path.read_text())
            if record["digest"] != digest: raise ControlError("another round is already active")
            existing = _remote_user_with_text(messages, text, record.get("preceding_message_id"))
            if existing:
                record["user_message_id"] = existing.get("info", {}).get("id"); record["delivered_at"] = now(); atomic_json(record_path, record)
                return record
            raise ControlError("round delivery is pending reconciliation; refusing duplicate send", 75)
        if require_empty:
            if messages: raise ControlError("prepared session is not empty; refusing to start")
        else:
            _, latest = live_boundary(context, allow_length=require_finish == "length")
            if latest.get("info", {}).get("finish") != require_finish:
                raise ControlError(f"latest assistant message did not finish with {require_finish}")
        number = int(state["next_round"]); filename = f"{number:03d}-{kind}.json"
        preceding_message_id = messages[-1].get("info", {}).get("id") if messages else None
        record = {"schema": "telora.opencode-round/v1", "number": number, "kind": kind, "digest": digest,
                  "text": text, "intent_at": now(), "delivered_at": None, "user_message_id": None,
                  "assistant_message_id": None, "completed_at": None, "finish": None, "source": source,
                  "preceding_message_id": preceding_message_id}
        (context.root / "rounds").mkdir(exist_ok=True); atomic_json(context.root / "rounds" / filename, record)
        state["active_round"] = {"number": number, "file": filename, "digest": digest}; state["phase"] = "active"
        state["next_round"] = number + 1
        if kind == "initial" and not state["started_at"]: state["started_at"] = now()
        save_state(context.root, state)
        client.prompt(text)
        existing = None
        for _ in range(10):
            messages = client.messages(); existing = _remote_user_with_text(messages, text, preceding_message_id)
            if existing: break
            time.sleep(.05)
        if existing:
            record["user_message_id"] = existing.get("info", {}).get("id"); record["delivered_at"] = now(); atomic_json(context.root / "rounds" / filename, record)
        return record


def reconcile(context: Context) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    messages = context.client().messages()
    with locked(context.root):
        state = load_state(context.root); active = state.get("active_round")
        if not active: return state, messages
        path = context.root / "rounds" / active["file"]; record = json.loads(path.read_text())
        user = _remote_user_with_text(messages, record["text"], record.get("preceding_message_id"))
        if user:
            record["user_message_id"] = user.get("info", {}).get("id"); record["delivered_at"] = record.get("delivered_at") or now()
            user_index = messages.index(user)
            assistants = [m for m in messages[user_index + 1:] if m.get("info", {}).get("role") == "assistant"]
            if assistants:
                last = assistants[-1]; info = last.get("info", {})
                if info.get("time", {}).get("completed") is not None and info.get("finish") in ("stop", "length"):
                    record["assistant_message_id"] = info.get("id"); record["completed_at"] = info.get("time", {}).get("completed"); record["finish"] = info.get("finish")
                    state["phase"] = "idle"; state["active_round"] = None
            atomic_json(path, record); save_state(context.root, state)
        return state, messages


def run_validation(context: Context) -> list[dict[str, Any]]:
    directory = context.root / "result" / "validation"; directory.mkdir(parents=True, exist_ok=True)
    results = []
    for item in context.manifest.validation:
        started = now(); command = resolve_command(item["command"], Path(context.state["workspace"])); result = subprocess.run(command, cwd=context.state["workspace"], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        record = {"name": item["name"], "command": item["command"], "started_at": started, "finished_at": now(), "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
        atomic_json(directory / f"{item['name']}.json", record); results.append(record)
    return results


def copy_archive(context: Context, destination: Path) -> None:
    workspace = Path(context.state["workspace"]).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{destination.name}.", dir=destination.parent))
    try:
        for relative in context.manifest.archive:
            source = workspace / relative
            if source.is_symlink() or not source.resolve().is_relative_to(workspace): raise ControlError(f"unsafe archive path: {relative}")
            target = staging / relative
            if source.is_dir():
                for child in source.rglob("*"):
                    if child.is_symlink() or not child.resolve().is_relative_to(workspace):
                        raise ControlError(f"unsafe symlink in archive path: {child.relative_to(workspace)}")
                shutil.copytree(source, target, symlinks=False)
            elif source.is_file(): target.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(source, target)
            else: raise ControlError(f"missing archive path: {relative}")
        if destination.exists(): shutil.rmtree(destination)
        os.replace(staging, destination)
    finally:
        if staging.exists(): shutil.rmtree(staging)


def export_session(context: Context, session_id: str) -> Any:
    error = ""
    for attempt in range(3):
        with tempfile.TemporaryFile() as output:
            result = subprocess.run(
                [*resolve_cli("opencode"), "export", session_id, "--pure"], cwd=context.state["workspace"],
                stdout=output, stderr=subprocess.PIPE,
            )
            output.seek(0); exported = output.read()
        if result.returncode:
            error = f"opencode session export failed: {result.stderr.decode(errors='replace').strip()}"
        else:
            try:
                return json.loads(exported)
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                error = f"opencode export returned malformed JSON: {exc}"
        if attempt < 2: time.sleep(0.25)
    raise ControlError(error, 70)


def finish(context: Context) -> dict[str, Any]:
    state, messages = reconcile(context)
    if state["active_round"]: raise ControlError("execution still has an active round", 75)
    live_boundary(context)
    with locked(context.root):
        state = load_state(context.root); state["phase"] = "finishing"; save_state(context.root, state)
    validation = run_validation(context)
    if any(v["exit"] and next((x.get("required", True) for x in context.manifest.validation if x["name"] == v["name"]), True) for v in validation):
        with locked(context.root): state = load_state(context.root); state["phase"] = "failed"; save_state(context.root, state)
        raise ControlError("required validation failed", 1)
    try:
        result = context.root / "result"; copy_archive(context, result / "workspace")
        raw_session = export_session(context, state["session_id"])
        atomic_json(result / "session.json", raw_session)
        atomic_json(result / "messages.json", messages)
        final_state = dict(state); final_state["phase"] = "finished"; final_state["finished_at"] = now()
        document = normalized(final_state, messages, context.client().status(), context.rounds(), context.manifest.observe, validation)
        atomic_json(result / "query.json", document)
        summary = document["summary"]
        atomic_write(result / "RUNLOG.md", ("# Run log\n\n```json\n" + json.dumps(summary, indent=2) + "\n```\n").encode())
        atomic_write(result / "SUMMARY.md", f"# {state['exec_name']} summary\n\nExecution data was frozen at {now()}.\n".encode())
    except Exception:
        with locked(context.root):
            current = load_state(context.root); current["phase"] = "idle"; save_state(context.root, current)
        raise
    with locked(context.root):
        state = load_state(context.root); state["phase"] = "finished"; state["finished_at"] = final_state["finished_at"]; save_state(context.root, state)
    return document


def safe_cleanup(state: dict[str, Any]) -> None:
    run_root = Path(state["run_root"]); workspace = Path(state["workspace"])
    if run_root.parent != Path("/tmp") or not run_root.name.startswith(f"oc-exp-{state['exec_name']}-") or workspace != run_root / "ws" or run_root.is_symlink():
        raise ControlError("refusing unsafe temporary cleanup")
    if run_root.exists(): shutil.rmtree(run_root)
