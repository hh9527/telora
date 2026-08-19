from __future__ import annotations

import argparse
import json
import sys
import tempfile
import time
from pathlib import Path, PurePosixPath
from typing import Any

from .config import ControlError, load_manifest, repository_root
from .context import Context, resolve
from .lifecycle import publish_workflow_artifact, request_start, send_round, verify_prepared
from .metrics import collect_metrics
from .state import create_run_config, load_run_config, load_state, run_config_path
from .task_cli import TaskError, remove_artifact, task_records, workflow_status


def emit(value: object) -> None:
    print(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True))


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="oc-ctl", description="Control a named artifact-DAG experiment.")
    commands = root.add_subparsers(dest="command", required=True)
    for name in ("start", "stat", "status"):
        item = commands.add_parser(name)
        item.add_argument("test_id")
    update = commands.add_parser("update")
    update.add_argument("test_id")
    update.add_argument("files", nargs="+")
    publish = commands.add_parser("publish")
    publish.add_argument("test_id")
    publish.add_argument("artifacts", nargs="+")
    return root


def _workspace(context: Context) -> Path:
    value = context.state.get("workspace")
    if not value:
        raise ControlError(f"execution is {context.state['phase']}; workspace is not ready", 75)
    return Path(value)


def _safe_relative(value: str, where: str) -> Path:
    path = PurePosixPath(value)
    if path.is_absolute() or not value or any(part in ("", ".", "..") for part in value.split("/")):
        raise ControlError(f"unsafe {where}: {value}", 64)
    return Path(*path.parts)


def _controller_repo() -> Path:
    return repository_root(Path(__file__).resolve().parent)


def _selected_plan(repo: Path, cwd: Path | None = None) -> str:
    current = (cwd or Path.cwd()).resolve()
    plans = repo / "experiments"
    for candidate in (current, *current.parents):
        if candidate.parent == plans and (candidate / "experiment.json").is_file():
            load_manifest(repo, candidate.name)
            return candidate.name
    raise ControlError("Host must run start from inside the autonomously selected experiment plan", 66)


def _automatic_port(repo: Path) -> int:
    used = set()
    for path in (repo / "target" / "exp").glob("*/config.json"):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(value, dict) and isinstance(value.get("port"), int):
            used.add(value["port"])
    return next(port for port in range(4100, 65536) if port not in used)


def _configure_start(repo: Path, test_id: str) -> dict[str, Any]:
    path = run_config_path(repo, test_id)
    if path.is_file():
        return load_run_config(repo, test_id)
    return create_run_config(repo, test_id, _selected_plan(repo), _automatic_port(repo))


def _update(context: Context, values: list[str]) -> list[dict[str, Any]]:
    workspace = _workspace(context)
    results = []
    for value in values:
        if "=" not in value:
            raise ControlError("update operands must be <dest-file>=<src-file> or <dest-file>=!", 64)
        destination_name, source_name = value.split("=", 1)
        destination = workspace / _safe_relative(destination_name, "destination")
        if source_name == "!":
            destination.unlink(missing_ok=True)
            results.append({"destination": destination_name, "removed": True})
            continue
        source = Path(source_name).expanduser()
        if not source.is_absolute():
            source = Path.cwd() / source
        if not source.is_file():
            raise ControlError(f"missing source file: {source_name}", 66)
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=f".{destination.name}.", delete=False) as output:
            temporary = Path(output.name)
            output.write(source.read_bytes())
        temporary.replace(destination)
        results.append({"destination": destination_name, "source": source_name,
                        "bytes": destination.stat().st_size})
    return results


def _publish(context: Context, values: list[str]) -> list[dict[str, Any]]:
    workflow = context.state.get("workflow")
    workspace = _workspace(context)
    if not workflow:
        raise ControlError("execution workflow is not prepared", 75)
    results = []
    for value in values:
        remove = value.endswith("=!")
        name = value[:-2] if remove else value
        if not name:
            raise ControlError(f"invalid artifact operand: {value}", 64)
        if remove:
            try:
                results.append(remove_artifact(workspace, workflow, name))
            except TaskError as exc:
                raise ControlError(str(exc), exc.code) from None
        else:
            results.append(publish_workflow_artifact(context, name, "publish"))
    return results


def _live_children(context: Context) -> tuple[list[dict[str, Any]], dict[str, list[dict[str, Any]]], dict[str, Any]]:
    client = context.client()
    children = client.children()
    messages = {child["id"]: client.session_messages(child["id"])
                for child in children if isinstance(child.get("id"), str)}
    return children, messages, client.statuses()


def _metrics(context: Context) -> tuple[dict[str, Any], dict[str, Any]]:
    workspace = _workspace(context)
    children, messages, statuses = _live_children(context)
    records = task_records(workspace)
    metrics = collect_metrics(
        context.state["exec_name"], context.state["phase"], workspace, children,
        messages.__getitem__, context.state.get("metrics", context.manifest.metrics), records,
    )
    agents = []
    by_role = {role["agent"]: role for role in metrics["roles"]}
    for role in context.state.get("workflow", {}).get("roles", []):
        child = next((item for item in children if item.get("agent") == role), None)
        role_metrics = by_role.get(role, {})
        agents.append({
            "role": role,
            "session_id": child.get("id") if child else None,
            "state": (statuses.get(child.get("id"), {"type": "stopped"}) if child else {"type": "not-started"}),
            "latest_task": role_metrics.get("latest_task"),
        })
    return metrics, {"agents": agents, "records": records}


def _status(context: Context) -> dict[str, Any]:
    if context.state["phase"] in ("waiting", "preparing"):
        return {"test_id": context.state["exec_name"], "phase": context.state["phase"],
                "workspace": context.state.get("workspace"), "artifacts": {}, "agents": []}
    metrics, detail = _metrics(context)
    workflow = context.state.get("workflow")
    artifacts = workflow_status(_workspace(context), workflow)
    return {
        "test_id": context.state["exec_name"],
        "phase": context.state["phase"],
        "workspace": context.state.get("workspace"),
        "artifacts": artifacts,
        "agents": detail["agents"],
        "tokens": metrics["aggregate"]["tokens"],
    }


def _start(context: Context) -> dict[str, Any]:
    request_start(context.root)
    deadline = time.monotonic() + 600
    while True:
        context.state = load_state(context.root)
        if context.state["phase"] == "failed":
            raise ControlError("oc-run failed while preparing the execution")
        if context.state["phase"] in ("ready", "active", "idle"):
            try:
                context.client().health()
                break
            except ControlError:
                pass
        if time.monotonic() >= deadline:
            raise ControlError("timed out waiting for oc-run to enter the TUI", 75)
        time.sleep(.1)
    verify_prepared(context.manifest, context.state)
    workflow = context.state.get("workflow")
    if workflow:
        for artifact in workflow["start_artifacts"]:
            publish_workflow_artifact(context, artifact, "start", once=f"workflow_started_{artifact}")
    initial = [record for record in context.rounds() if record.get("kind") == "initial"]
    if initial and initial[0].get("user_message_id"):
        return initial[0]
    return send_round(context, "initial", context.manifest.prompts["start"], require_empty=True)


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "start":
            repo = _controller_repo()
            _configure_start(repo, args.test_id)
            deadline = time.monotonic() + 30
            state_path = run_config_path(repo, args.test_id).parent / "state.json"
            while not state_path.is_file():
                if time.monotonic() >= deadline:
                    raise ControlError("timed out waiting for oc-run to consume its configuration", 75)
                time.sleep(.1)
            context = resolve(args.test_id, repo)
            emit(_start(context))
            return 0
        context = resolve(args.test_id, _controller_repo())
        if args.command == "status":
            emit(_status(context))
        elif args.command == "stat":
            emit(_metrics(context)[0])
        elif args.command == "update":
            emit(_update(context, args.files))
        elif args.command == "publish":
            emit(_publish(context, args.artifacts))
        return 0
    except (ControlError, TaskError) as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr)
        return getattr(exc, "code", 65)
    except (FileNotFoundError, PermissionError, UnicodeError, json.JSONDecodeError) as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr)
        return 66
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
