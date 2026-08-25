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
from .lifecycle import opencode_environment
from .state import (
    create_runner_config,
    runner_workspace_path,
)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(prog="oc-run", description="Run a stable headless experiment daemon.")
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
                f"Lab {test_id} is ready on port {args.port}. "
                f"Keep this process running; Host controls experiments with oc-ctl.",
                flush=True,
            )
            returncode = server.wait()
            if returncode:
                raise ControlError(
                    f"opencode daemon exited with status {returncode}; "
                    f"see {runner_workspace.parent / 'runner.log'}",
                    70,
                )
            return 0
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
