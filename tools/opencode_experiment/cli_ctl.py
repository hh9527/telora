from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from .config import ControlError, repository_root, validate_identifier
from .context import Context, resolve
from .external import resolve_capabilities, resolve_cli
from .lifecycle import finish, live_boundary, publish_workflow_node, quiesce_workflow, reconcile, request_start, run_validation, safe_cleanup, send_round, verify_prepared
from .metrics import collect_metrics
from .observe import failures, latest_assistant, normalized, recent, text_parts, timeline
from .query import run_query, select_engine
from .reporting import submit_report
from .state import atomic_json, load_state, locked, save_state
from .task_cli import workflow_status
from .watch import watch_progress


def emit(value: object) -> None:
    print(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True))


def count(value: str, maximum: int) -> int:
    try: result = int(value)
    except ValueError: raise argparse.ArgumentTypeError("must be an integer") from None
    if not 1 <= result <= maximum: raise argparse.ArgumentTypeError(f"must be from 1 through {maximum}")
    return result


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="oc-ctl", description="Control and observe named opencode experiments.")
    commands = root.add_subparsers(dest="command", required=True)
    commands.add_parser("doctor")
    for name in ("workspace", "start", "status", "snapshot", "events", "files", "failures", "audit", "answer", "continue", "validate", "export", "finish", "retire", "children", "tree", "stats"):
        item = commands.add_parser(name); item.add_argument("exec_name")
        if name == "answer": item.add_argument("--json", action="store_true", dest="as_json")
    for name, default, maximum in (("recent", 3, 20), ("timeline", 8, 50)):
        item = commands.add_parser(name); item.add_argument("exec_name"); item.add_argument("count", nargs="?", default=default, type=lambda x, m=maximum: count(x, m))
    ask = commands.add_parser("ask"); ask.add_argument("exec_name"); group = ask.add_mutually_exclusive_group(required=True); group.add_argument("message", nargs="?"); group.add_argument("--file")
    child = commands.add_parser("child-recent"); child.add_argument("exec_name"); child.add_argument("session_id"); child.add_argument("count", nargs="?", default=3, type=lambda x: count(x, 20))
    child_continue = commands.add_parser("child-continue"); child_continue.add_argument("exec_name"); child_continue.add_argument("session_id")
    child_ask = commands.add_parser("child-ask"); child_ask.add_argument("exec_name"); child_ask.add_argument("session_id"); child_ask.add_argument("--agent", required=True); group = child_ask.add_mutually_exclusive_group(required=True); group.add_argument("message", nargs="?"); group.add_argument("--file")
    child_abort = commands.add_parser("child-abort"); child_abort.add_argument("exec_name"); child_abort.add_argument("session_id")
    query = commands.add_parser("query"); query.add_argument("exec_name"); group = query.add_mutually_exclusive_group(required=True); group.add_argument("expression", nargs="?"); group.add_argument("--file"); query.add_argument("--raw-output", action="store_true")
    watch = commands.add_parser("watch"); watch.add_argument("exec_name"); watch.add_argument("--debounce", type=lambda x: count(x, 3600), default=30); watch.add_argument("--timeout", type=lambda x: count(x, 86400), default=300)
    report = commands.add_parser("report"); report.add_argument("exec_name"); report.add_argument("--body-file", required=True, type=Path)
    ready = commands.add_parser("ready"); ready.add_argument("exec_name"); ready.add_argument("node")
    feedback = commands.add_parser("feedback"); feedback.add_argument("exec_name"); feedback.add_argument("node"); feedback.add_argument("--body-file", required=True, type=Path)
    tasks = commands.add_parser("tasks"); tasks.add_argument("exec_name")
    return root


def live_document(context: Context) -> tuple[dict, list]:
    state, messages = reconcile(context); context.state = state
    return normalized(state, messages, context.client().status(), context.rounds(), context.manifest.observe), messages


def doctor() -> dict:
    repo = repository_root(); git = resolve_cli("git"); revision = subprocess.run([*git, "rev-parse", "HEAD"], cwd=repo, text=True, stdout=subprocess.PIPE).stdout.strip()
    result = {"python": sys.version.split()[0], "repository": str(repo), "revision": revision,
              "commands": {},
              "override": os.environ.get("OC_QUERY_ENGINE"), "query_engine": None}
    for capability, choices in {"git": ("git",), "opencode": ("opencode",), "query": ("jaq", "jq")}.items():
        try: result["commands"][capability] = list(resolve_capabilities({capability: choices})[capability])
        except ControlError: result["commands"][capability] = None
    try: result["query_engine"] = select_engine()[0]
    except ControlError: pass
    if result["commands"]["opencode"]:
        version = subprocess.run([*result["commands"]["opencode"], "--version"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        result["opencode_version"] = version.stdout.strip()
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "doctor": emit(doctor()); return 0
        context = resolve(args.exec_name)
        exec_name = args.exec_name
        if args.command == "watch":
            emit(watch_progress(context, args.debounce, args.timeout)); return 0
        if args.command == "report":
            value = submit_report(context, args.body_file)
            emit({"exec_name": exec_name, "report": value["number"], "status": value["status"]})
            return 1 if value["status"] == "error" else 0
        if args.command in ("child-continue", "child-ask", "child-abort"):
            children = context.client().children()
            if not any(child.get("id") == args.session_id for child in children):
                raise ControlError("session is not a direct child of this execution", 64)
            if args.command == "child-abort":
                context.client().abort_session(args.session_id)
                emit({"exec_name": exec_name, "session_id": args.session_id, "aborted": True}); return 0
            text = context.manifest.prompts["continue"] if args.command == "child-continue" else (Path(args.file).read_text(encoding="utf-8") if args.file else args.message)
            if not text or not text.strip(): raise ControlError("child message must not be empty", 64)
            agent = None if args.command == "child-continue" else validate_identifier(args.agent, "agent")
            context.client().prompt_session(args.session_id, text, agent)
            emit({"exec_name": exec_name, "session_id": args.session_id, "agent": agent, "text": text}); return 0
        if args.command == "workspace": print(context.state["workspace"]); return 0
        if args.command == "start":
            request_start(context.root)
            deadline = time.monotonic() + 600
            while True:
                context.state = load_state(context.root)
                if context.state["phase"] == "failed": raise ControlError("oc-run failed while preparing the execution")
                if context.state["phase"] in ("ready", "active", "idle"):
                    try:
                        context.client().health()
                        break
                    except ControlError:
                        pass
                if time.monotonic() >= deadline: raise ControlError("timed out waiting for oc-run to enter the TUI", 75)
                time.sleep(.1)
            verify_prepared(context.manifest, context.state)
            if context.state.get("workflow"):
                for node_id in context.state["workflow"]["start_nodes"]:
                    publish_workflow_node(context, node_id, "start", once=f"workflow_started_{node_id}")
            initial = [record for record in context.rounds() if record.get("kind") == "initial"]
            if initial and initial[0].get("user_message_id"):
                emit(initial[0]); return 0
            emit(send_round(context, "initial", context.manifest.prompts["start"], require_empty=True)); return 0
        if args.command in ("status", "snapshot", "recent", "timeline", "files", "failures", "audit", "answer", "query", "children", "tree", "child-recent", "stats"):
            if context.state["phase"] in ("waiting", "preparing"):
                if args.command != "status": raise ControlError(f"execution is {context.state['phase']}; only status and start are available", 75)
                emit({"workspace": context.state.get("workspace"), "session_id": context.state.get("session_id"),
                      "phase": context.state["phase"], "status": {"type": context.state["phase"]}})
                return 0
            frozen = context.state["phase"] in ("finished", "retired") or not Path(context.state["workspace"]).exists()
            if args.command == "stats":
                if frozen:
                    child_records = json.loads((context.root / "result" / "children.json").read_text(encoding="utf-8"))
                    children = []
                    child_messages = {}
                    for record in child_records:
                        session_id = record["session_id"]
                        exported = json.loads((context.root / "result" / "children" / f"{session_id}.json").read_text(encoding="utf-8"))
                        info = dict(exported.get("info", {}))
                        info.setdefault("id", session_id)
                        info.setdefault("title", record.get("title"))
                        children.append(info)
                        messages_path = context.root / "result" / "children" / f"{session_id}.messages.json"
                        child_messages[session_id] = json.loads(messages_path.read_text(encoding="utf-8"))
                    workspace = context.root / "result" / "workspace"
                    load_messages = child_messages.__getitem__
                else:
                    client = context.client()
                    children = client.children()
                    workspace = Path(context.state["workspace"])
                    load_messages = client.session_messages
                emit(collect_metrics(exec_name, context.state["phase"], workspace, children,
                                     load_messages, context.state.get("metrics", context.manifest.metrics)))
                return 0
            if frozen:
                document = json.loads((context.root / "result" / "query.json").read_text(encoding="utf-8")); messages = document["messages"]
                document["state"] = context.state
            else: document, messages = live_document(context)
            if args.command == "status": emit({"workspace": context.state["workspace"], "session_id": context.state["session_id"], "phase": document["state"]["phase"], "status": document["status"]})
            elif args.command == "snapshot": emit({"status": document["status"], "recent": recent(messages, 3), "files": document["files"]})
            elif args.command == "recent": emit(recent(messages, args.count))
            elif args.command == "timeline": emit(timeline(messages, args.count))
            elif args.command == "files": emit(document["files"])
            elif args.command == "failures": emit(document["failures"])
            elif args.command == "audit": emit({"summary": document["summary"], "user_messages": [{"message_id": m.get("info", {}).get("id"), "created": m.get("info", {}).get("time", {}).get("created"), "synthetic": m.get("info", {}).get("synthetic", False), "text": text_parts(m)} for m in messages if m.get("info", {}).get("role") == "user"], "failures": document["failures"]})
            elif args.command == "answer":
                latest = latest_assistant(messages)
                if not latest or latest.get("info", {}).get("time", {}).get("completed") is None: raise ControlError("no completed assistant answer")
                info = latest["info"]; value = {"exec_name": exec_name, "message_id": info.get("id"), "completed": info.get("time", {}).get("completed"), "finish": info.get("finish"), "text": "\n".join(text_parts(latest))}
                emit(value) if args.as_json else print(value["text"])
            elif args.command in ("children", "tree"):
                if context.state["phase"] in ("finished", "retired"):
                    emit(json.loads((context.root / "result" / "children.json").read_text(encoding="utf-8")))
                else:
                    emit(context.client().children())
            elif args.command == "child-recent":
                if context.state["phase"] in ("finished", "retired"):
                    path = context.root / "result" / "children" / f"{args.session_id}.messages.json"
                    emit(recent(json.loads(path.read_text(encoding="utf-8")), args.count))
                else:
                    emit(recent(context.client().session_messages(args.session_id), args.count))
            else:
                return run_query(json.dumps(document, ensure_ascii=False), args.expression, args.file, args.raw_output)
            return 0
        if args.command == "events":
            for event in context.client().events():
                properties = event.get("properties", {})
                if properties.get("sessionID") != context.state["session_id"]: continue
                if event.get("type") in ("session.status", "session.error", "message.updated") or (event.get("type") == "message.part.updated" and properties.get("part", {}).get("type") == "tool" and properties.get("part", {}).get("state", {}).get("status") in ("completed", "error")):
                    emit(event)
            return 0
        if args.command == "ready":
            emit(publish_workflow_node(context, args.node, "ready")); return 0
        if args.command == "feedback":
            content = args.body_file.read_bytes()
            emit(publish_workflow_node(context, args.node, "feedback", content=content)); return 0
        if args.command == "tasks":
            if not context.state.get("workflow") or not context.state.get("workspace"):
                raise ControlError("execution workflow is not prepared", 75)
            emit(workflow_status(Path(context.state["workspace"]), context.state["workflow"])); return 0
        if args.command == "ask":
            text = Path(args.file).read_text(encoding="utf-8") if args.file else args.message
            if not text: raise ControlError("ask message must not be empty", 64)
            emit(send_round(context, "ask", text)); return 0
        if args.command == "continue":
            _, latest = live_boundary(context, allow_length=True)
            if latest.get("info", {}).get("finish") != "length": raise ControlError("latest assistant message did not finish at length")
            emit(send_round(context, "continue", context.manifest.prompts["continue"], require_finish="length")); return 0
        if args.command == "validate":
            values = run_validation(context); emit(values); return 1 if any(v["exit"] for v in values) else 0
        if args.command == "export":
            from .lifecycle import export_session
            value = export_session(context, context.state["session_id"])
            path = context.root / "session-export.json"; atomic_json(path, value)
            emit({"path": str(path), "bytes": path.stat().st_size,
                  "messages": len(value.get("messages", [])) if isinstance(value, dict) else None})
            return 0
        if args.command == "finish":
            quiesce_workflow(context)
            finish(context); print(f"Execution {exec_name} is frozen. You may exit the TUI."); return 0
        if args.command == "retire":
            if context.state["phase"] not in ("finished", "retired"): raise ControlError("only a finished execution can be retired")
            if not all((context.root / "result" / name).is_file() for name in ("query.json", "session.json", "messages.json")): raise ControlError("frozen query evidence is incomplete")
            safe_cleanup(context.state)
            with locked(context.root): state = load_state(context.root); state["phase"] = "retired"; save_state(context.root, state)
            print(f"Execution {exec_name} retired."); return 0
        raise ControlError(f"unsupported command: {args.command}", 64)
    except ControlError as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr); return exc.code
    except (FileNotFoundError, PermissionError, UnicodeError, json.JSONDecodeError) as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr); return 66
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
