from __future__ import annotations

import argparse
import socket
import subprocess
import sys
import time
from pathlib import Path

from .client import Client
from .config import ControlError, repository_root, validate_identifier
from .external import resolve_cli
from .lifecycle import create_execution_session, opencode_environment, prepare, reserve, safe_cleanup, start_requested
from .state import (
    create_runner_config,
    load_run_config,
    load_state,
    run_config_path,
    runner_workspace_path,
)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-run", description="Wait for and run a Host-configured experiment TUI.")
    value.add_argument("test_id")
    value.add_argument("port", type=int)
    return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        opencode = resolve_cli("opencode")
        test_id = validate_identifier(args.test_id, "test-id")
        if not 1 <= args.port <= 65535:
            raise ControlError("port must be from 1 through 65535", 64)
        repo = repository_root(Path(__file__).resolve().parent)
        config_path = run_config_path(repo, test_id)
        runner_workspace = runner_workspace_path(repo, test_id)
        runner_workspace.mkdir(parents=True, exist_ok=True)
        with socket.socket() as reservation:
            try:
                reservation.bind(("127.0.0.1", args.port))
            except OSError as exc:
                raise ControlError(f"cannot reserve runner port {args.port}: {exc}", 69) from None
        log = (runner_workspace.parent / "runner.log").open("ab")
        server = subprocess.Popen(
            [*opencode, "serve", "--hostname", "127.0.0.1", "--port", str(args.port), "--pure"],
            cwd=runner_workspace,
            env=opencode_environment({}),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            client = Client(f"http://127.0.0.1:{args.port}", str(runner_workspace), timeout=0.1)
            deadline = time.monotonic() + 10
            while True:
                if server.poll() is not None:
                    raise ControlError(
                        f"opencode daemon exited while establishing the runner; see {runner_workspace.parent / 'runner.log'}",
                        70,
                    )
                try:
                    client.health()
                    break
                except ControlError:
                    if time.monotonic() >= deadline:
                        raise ControlError(
                            f"timed out connecting to runner daemon; see {runner_workspace.parent / 'runner.log'}",
                            69,
                        ) from None
                    time.sleep(.1)
            create_runner_config(repo, test_id, args.port)
            print(
                f"Execution {test_id} started an empty lab on port {args.port} and is waiting for Host "
                f"configuration: {config_path}",
                flush=True,
            )
            while not config_path.is_file():
                time.sleep(.25)
            config = load_run_config(repo, test_id)
            if config["port"] != args.port:
                raise ControlError("Host configuration does not match the reserved runner port", 64)
            plan_id, port = config["plan_id"], config["port"]
            from_test_id = config.get("from_test_id")
            root, state = reserve(plan_id, test_id, port, from_test_id=from_test_id)
            if state["phase"] == "waiting":
                print(f"Execution {test_id} is waiting for: ./oc-ctl start {test_id} {plan_id}", flush=True)
                while not start_requested(root):
                    time.sleep(.25)
            root, state, _ = prepare(plan_id, test_id, port, from_test_id=from_test_id)
            state = create_execution_session(root, state, f"{plan_id} / {test_id} (ready)")
            workspace, server_url, session_id = state["workspace"], state["server_url"], state["session_id"]
            print(f"Workspace ready: {workspace}\nEmpty session ready: {session_id}", flush=True)
            result = subprocess.run(
                [*opencode, "attach", server_url, "--dir", workspace, "--session", session_id, "--pure"],
                env=opencode_environment(state),
            )
            state = load_state(root)
            if state["phase"] in ("finished", "retired") and all((root / "result" / name).is_file() for name in ("query.json", "session.json", "messages.json")):
                safe_cleanup(state)
                print(f"Execution {test_id} is frozen; temporary workspace removed.")
            else:
                print(f"Execution {test_id} remains resumable in phase {state['phase']}.")
            return result.returncode
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            log.close()
    except ControlError as exc:
        print(f"oc-run: {exc}", file=sys.stderr); return exc.code
    except KeyboardInterrupt: return 130


if __name__ == "__main__": raise SystemExit(main())
