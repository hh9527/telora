from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

from .client import Client
from .config import ControlError, repository_root, validate_identifier
from .external import resolve_cli
from .lifecycle import create_empty_session, opencode_environment, prepare, reserve, safe_cleanup, start_requested
from .state import load_run_config, load_state, run_config_path


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-run", description="Wait for and run a Host-configured experiment TUI.")
    value.add_argument("test_id")
    return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        opencode = resolve_cli("opencode")
        test_id = validate_identifier(args.test_id, "test-id")
        repo = repository_root(Path(__file__).resolve().parent)
        config_path = run_config_path(repo, test_id)
        print(f"Execution {test_id} is waiting for Host configuration: {config_path}", flush=True)
        while not config_path.is_file():
            time.sleep(.25)
        config = load_run_config(repo, test_id)
        plan_id, port = config["plan_id"], config["port"]
        root, state = reserve(plan_id, test_id, port)
        if state["phase"] == "waiting":
            print(f"Execution {test_id} is waiting for: ./oc-ctl start {test_id}", flush=True)
            while not start_requested(root):
                time.sleep(.25)
        root, state, _ = prepare(plan_id, test_id, port)
        state = create_empty_session(root, state, f"{plan_id} / {test_id} (ready)")
        workspace, server_url, session_id = state["workspace"], state["server_url"], state["session_id"]
        port = int(server_url.rsplit(":", 1)[1])
        print(f"Workspace ready: {workspace}\nEmpty session ready: {session_id}", flush=True)
        live = False
        try:
            Client(server_url, workspace, session_id, timeout=1).health(); live = True
        except ControlError: pass
        command = ([*opencode, "attach", server_url, "--dir", workspace, "--session", session_id, "--pure"] if live else
                   [*opencode, workspace, "--hostname", "127.0.0.1", "--port", str(port), "--session", session_id, "--pure"])
        result = subprocess.run(command, env=opencode_environment(state))
        state = load_state(root)
        if state["phase"] in ("finished", "retired") and all((root / "result" / name).is_file() for name in ("query.json", "session.json", "messages.json")):
            safe_cleanup(state)
            print(f"Execution {test_id} is frozen; temporary workspace removed.")
        else:
            print(f"Execution {test_id} remains resumable in phase {state['phase']}.")
        return result.returncode
    except ControlError as exc:
        print(f"oc-run: {exc}", file=sys.stderr); return exc.code
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
