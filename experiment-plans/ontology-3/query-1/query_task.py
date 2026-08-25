#!/usr/bin/env python3
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
TELORA = ROOT / "bin" / "telora"
ENGINE = ROOT / "query-1"
CASE_NAME = re.compile(r"[a-z0-9][a-z0-9-]*")
PROBLEM_ID = re.compile(r"[0-9]{4}")


def run_query(source: Path) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        [
            str(TELORA),
            "run",
            "query",
            "-C",
            str(ENGINE),
            "--source",
            f"request=file+json://{source.resolve()}",
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
    )


def relay(result: subprocess.CompletedProcess[bytes]) -> None:
    sys.stdout.buffer.write(result.stdout)
    sys.stderr.buffer.write(result.stderr)


def require_file(path: Path) -> None:
    if not path.is_file():
        raise SystemExit(f"query-task: missing JSON input: {path.relative_to(ROOT)}")


def a4(action: str, name: str) -> int:
    if not action:
        print("A4 actions: make-query | expect-invalid <name> | verify")
        return 0
    source = ROOT / "intent-1" / "intent.json"
    if action == "make-query" and not name:
        require_file(source)
        result = run_query(source)
        relay(result)
        return result.returncode
    if action == "expect-invalid" and CASE_NAME.fullmatch(name):
        source = ROOT / "intent-1" / "invalid" / f"{name}.json"
        require_file(source)
        result = run_query(source)
        relay(result)
        if result.returncode == 0:
            print(f"query-task: invalid case unexpectedly produced a Query: {name}", file=sys.stderr)
            return 1
        if not result.stderr:
            print(f"query-task: invalid case failed without diagnostics: {name}", file=sys.stderr)
            return 1
        return 0
    if action == "verify" and not name:
        require_file(source)
        first = run_query(source)
        if first.returncode != 0:
            relay(first)
            return first.returncode
        second = run_query(source)
        if second.returncode != 0:
            relay(second)
            return second.returncode
        if first.stdout != second.stdout:
            print("query-task: repeated lowering produced different output", file=sys.stderr)
            return 1
        sys.stdout.buffer.write(second.stdout)
        return 0
    print("query-task: usage: just a4 make-query | expect-invalid <name> | verify", file=sys.stderr)
    return 64


def a5(action: str, problem_id: str) -> int:
    if not action:
        print("A5 actions: make-query <problem-id>")
        return 0
    if action != "make-query" or not PROBLEM_ID.fullmatch(problem_id):
        print("query-task: usage: just a5 make-query <four-digit-problem-id>", file=sys.stderr)
        return 64
    source = ROOT / "query-1" / "answers" / f"{problem_id}.json"
    require_file(source)
    result = run_query(source)
    relay(result)
    return result.returncode


def main(arguments: list[str]) -> int:
    if len(arguments) != 3 or arguments[0] not in {"a4", "a5"}:
        print("query-task: invoked outside a supported role entry", file=sys.stderr)
        return 64
    role, action, value = arguments
    return a4(action, value) if role == "a4" else a5(action, value)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
