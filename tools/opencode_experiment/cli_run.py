from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

from .client import Client
from .config import ControlError
from .external import resolve_cli
from .lifecycle import create_empty_session, prepare, safe_cleanup
from .state import load_state


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-run", description="Prepare or resume an external opencode experiment TUI.")
    value.add_argument("plan_id"); value.add_argument("exec_name"); value.add_argument("--port", type=int)
    value.add_argument("--artifact", action="append", default=[], metavar="NAME=PATH")
    return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        opencode = resolve_cli("opencode")
        artifacts = {}
        for value in args.artifact:
            if "=" not in value: raise ControlError("--artifact must be NAME=PATH", 64)
            name, path = value.split("=", 1)
            if not name or not path or name in artifacts: raise ControlError(f"invalid artifact override: {value}", 64)
            artifacts[name] = path
        root, state, _ = prepare(args.plan_id, args.exec_name, args.port, artifacts)
        state = create_empty_session(root, state, f"{args.plan_id} / {args.exec_name} (ready)")
        workspace, server_url, session_id = state["workspace"], state["server_url"], state["session_id"]
        port = int(server_url.rsplit(":", 1)[1])
        print(f"Workspace ready: {workspace}\nEmpty session ready: {session_id}", flush=True)
        live = False
        try:
            Client(server_url, workspace, session_id, timeout=1).health(); live = True
        except ControlError: pass
        command = ([*opencode, "attach", server_url, "--dir", workspace, "--session", session_id, "--pure"] if live else
                   [*opencode, workspace, "--hostname", "127.0.0.1", "--port", str(port), "--session", session_id, "--pure"])
        result = subprocess.run(command)
        state = load_state(root)
        if state["phase"] in ("finished", "retired") and all((root / "result" / name).is_file() for name in ("query.json", "session.json", "messages.json")):
            safe_cleanup(state)
            print(f"Execution {args.exec_name} is frozen; temporary workspace removed.")
        else:
            print(f"Execution {args.exec_name} remains resumable in phase {state['phase']}.")
        return result.returncode
    except ControlError as exc:
        print(f"oc-run: {exc}", file=sys.stderr); return exc.code
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
