from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from .config import ControlError, repository_root, sha256
from .context import Context, resolve
from .external import resolve_capabilities, resolve_cli
from .lifecycle import finish, live_boundary, reconcile, run_validation, safe_cleanup, send_round, verify_prepared
from .observe import failures, latest_assistant, normalized, recent, text_parts, timeline
from .query import run_query, select_engine
from .state import atomic_json, atomic_write, load_state, locked, now, save_state


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
    doctor = commands.add_parser("doctor"); doctor.add_argument("exec_name", nargs="?")
    for name in ("workspace", "start", "status", "snapshot", "events", "files", "failures", "audit", "answer", "continue", "feedback-status", "validate", "export", "finish", "retire"):
        item = commands.add_parser(name); item.add_argument("exec_name")
        if name == "answer": item.add_argument("--json", action="store_true", dest="as_json")
    for name, default, maximum in (("recent", 3, 20), ("timeline", 8, 50)):
        item = commands.add_parser(name); item.add_argument("exec_name"); item.add_argument("count", nargs="?", default=default, type=lambda x, m=maximum: count(x, m))
    ask = commands.add_parser("ask"); ask.add_argument("exec_name"); group = ask.add_mutually_exclusive_group(required=True); group.add_argument("message", nargs="?"); group.add_argument("--file")
    feedback = commands.add_parser("feedback"); feedback.add_argument("exec_name"); feedback.add_argument("--source-exec"); feedback.add_argument("--source-round", type=int); feedback.add_argument("--source-message")
    query = commands.add_parser("query"); query.add_argument("exec_name"); group = query.add_mutually_exclusive_group(required=True); group.add_argument("expression", nargs="?"); group.add_argument("--file"); query.add_argument("--raw-output", action="store_true")
    return root


def live_document(context: Context) -> tuple[dict, list]:
    state, messages = reconcile(context); context.state = state
    return normalized(state, messages, context.client().status(), context.rounds(), context.manifest.observe), messages


def doctor(exec_name: str | None) -> dict:
    repo = repository_root(); git = resolve_cli("git"); revision = subprocess.run([*git, "rev-parse", "HEAD"], cwd=repo, text=True, stdout=subprocess.PIPE).stdout.strip()
    result = {"python": sys.version.split()[0], "repository": str(repo), "revision": revision,
              "commands": {},
              "override": os.environ.get("OC_QUERY_ENGINE"), "query_engine": None, "execution": None}
    for capability, choices in {"git": ("git",), "opencode": ("opencode",), "query": ("jaq", "jq")}.items():
        try: result["commands"][capability] = list(resolve_capabilities({capability: choices})[capability])
        except ControlError: result["commands"][capability] = None
    try: result["query_engine"] = select_engine()[0]
    except ControlError: pass
    if result["commands"]["opencode"]:
        version = subprocess.run([*result["commands"]["opencode"], "--version"], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        result["opencode_version"] = version.stdout.strip()
    if exec_name: result["execution"] = resolve(exec_name).state
    return result


def feedback_command(context: Context, args: argparse.Namespace) -> dict:
    verify_prepared(context.manifest, context.state)
    path = Path(context.state["workspace"]) / context.manifest.feedback["path"]
    if not path.is_file() or path.is_symlink(): raise ControlError(f"feedback file is missing: {path}", 66)
    text = path.read_text(encoding="utf-8")
    if not text.strip(): raise ControlError("feedback file is empty")
    digest = sha256(path); directory = context.root / "feedback"; directory.mkdir(exist_ok=True)
    previous = []
    for meta in sorted(directory.glob("*.json")):
        previous.append(json.loads(meta.read_text(encoding="utf-8")))
    delivered = [item for item in previous if item.get("digest") == digest and item.get("round") is not None]
    if delivered: raise ControlError("this feedback digest was already delivered")
    pending = next((item for item in previous if item.get("digest") == digest), None)
    number = pending["number"] if pending else len(previous) + 1
    frozen = directory / f"{number:03d}.md"
    if pending:
        if frozen.read_text(encoding="utf-8") != text: raise ControlError("pending feedback snapshot does not match live feedback")
    else:
        atomic_write(frozen, text.encode(), 0o444)
    source = {"exec_name": args.source_exec, "round": args.source_round, "message_id": args.source_message}
    metadata = pending or {"schema": "telora.opencode-feedback/v1", "number": number, "target": context.state["exec_name"], "digest": digest, "created_at": now(), "source": source, "round": None}
    if not pending: atomic_json(directory / f"{number:03d}.json", metadata)
    prompt = context.manifest.prompts["feedback"]
    record = send_round(context, "feedback", prompt, source=source)
    metadata["round"] = record["number"]; metadata["delivered_at"] = now(); atomic_json(directory / f"{number:03d}.json", metadata)
    return metadata


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "doctor": emit(doctor(args.exec_name)); return 0
        context = resolve(args.exec_name)
        if args.command == "workspace": print(context.state["workspace"]); return 0
        if args.command == "start":
            verify_prepared(context.manifest, context.state)
            initial = [record for record in context.rounds() if record.get("kind") == "initial"]
            if initial and initial[0].get("user_message_id"):
                emit(initial[0]); return 0
            emit(send_round(context, "initial", context.manifest.prompts["start"], require_empty=True)); return 0
        if args.command in ("status", "snapshot", "recent", "timeline", "files", "failures", "audit", "answer", "query"):
            if context.state["phase"] in ("finished", "retired") or not Path(context.state["workspace"]).exists():
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
                info = latest["info"]; value = {"exec_name": args.exec_name, "message_id": info.get("id"), "completed": info.get("time", {}).get("completed"), "finish": info.get("finish"), "text": "\n".join(text_parts(latest))}
                emit(value) if args.as_json else print(value["text"])
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
        if args.command == "ask":
            text = Path(args.file).read_text(encoding="utf-8") if args.file else args.message
            if not text: raise ControlError("ask message must not be empty", 64)
            emit(send_round(context, "ask", text)); return 0
        if args.command == "continue":
            _, latest = live_boundary(context, allow_length=True)
            if latest.get("info", {}).get("finish") != "length": raise ControlError("latest assistant message did not finish at length")
            emit(send_round(context, "continue", context.manifest.prompts["continue"], require_finish="length")); return 0
        if args.command == "feedback": emit(feedback_command(context, args)); return 0
        if args.command == "feedback-status":
            values = [json.loads(p.read_text()) for p in sorted((context.root / "feedback").glob("*.json"))] if (context.root / "feedback").exists() else []
            emit(values); return 0
        if args.command == "validate":
            values = run_validation(context); emit(values); return 1 if any(v["exit"] for v in values) else 0
        if args.command == "export":
            from .lifecycle import export_session
            value = export_session(context, context.state["session_id"])
            path = context.root / "session-export.json"; atomic_json(path, value)
            emit({"path": str(path), "bytes": path.stat().st_size,
                  "messages": len(value.get("messages", [])) if isinstance(value, dict) else None})
            return 0
        if args.command == "finish": finish(context); print(f"Execution {args.exec_name} is frozen. You may exit the TUI."); return 0
        if args.command == "retire":
            if context.state["phase"] not in ("finished", "retired"): raise ControlError("only a finished execution can be retired")
            if not all((context.root / "result" / name).is_file() for name in ("query.json", "session.json", "messages.json")): raise ControlError("frozen query evidence is incomplete")
            safe_cleanup(context.state)
            with locked(context.root): state = load_state(context.root); state["phase"] = "retired"; save_state(context.root, state)
            print(f"Execution {args.exec_name} retired."); return 0
        raise ControlError(f"unsupported command: {args.command}", 64)
    except ControlError as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr); return exc.code
    except (FileNotFoundError, PermissionError, UnicodeError, json.JSONDecodeError) as exc:
        print(f"oc-ctl: {exc}", file=sys.stderr); return 66
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
