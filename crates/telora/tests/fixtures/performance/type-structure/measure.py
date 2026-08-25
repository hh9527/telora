#!/usr/bin/env python3

import argparse
import json
import resource
import statistics
import subprocess
import sys
import time
from pathlib import Path


WORKSPACE = Path(__file__).resolve().parent
REPOSITORY = Path(__file__).resolve().parents[6]
CASES = {
    "flat-functions": ("check", "@src/flat-functions"),
    "recursive-functions": ("check", "@src/recursive-functions"),
    "nested-functions": ("check", "@src/nested-functions"),
    "recursive-values-shallow": (
        "check",
        "@src/recursive-values-shallow",
    ),
    "recursive-values-growing": (
        "check",
        "@src/recursive-values-growing",
    ),
    "query-builder-check": ("check", "@src/query-builder"),
    "query-builder-show": (
        "show",
        "@src/query-builder",
        "-p",
        "definitely_missing_name",
    ),
}


def run_once(binary: Path, arguments: tuple[str, ...], timeout: float) -> tuple[float, float, float]:
    command = [str(binary), *arguments, "-C", str(WORKSPACE)]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
        check=False,
    )
    wall = time.perf_counter() - started
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if completed.returncode != 0:
        message = completed.stderr.strip() or "command produced no stderr"
        raise RuntimeError(f"{' '.join(command)} failed: {message}")
    return wall, after.ru_utime - before.ru_utime, after.ru_stime - before.ru_stime


def median(values: list[float]) -> float:
    return round(statistics.median(values), 6)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Measure the recursive type/value fixtures and emit JSON Lines."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=REPOSITORY / "target/release/telora",
        help="existing release telora binary",
    )
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=180.0)
    parser.add_argument("--no-warmup", action="store_true")
    parser.add_argument(
        "--fixture",
        action="append",
        choices=sorted(CASES),
        help="measure only the selected fixture; may be repeated",
    )
    arguments = parser.parse_args()
    binary = arguments.binary.resolve()
    if not binary.is_file():
        parser.error(f"binary does not exist: {binary}")
    if arguments.samples < 1:
        parser.error("--samples must be at least 1")

    selected = arguments.fixture or list(CASES)
    for name in selected:
        command = CASES[name]
        if not arguments.no_warmup:
            run_once(binary, command, arguments.timeout)
        samples = [
            run_once(binary, command, arguments.timeout)
            for _ in range(arguments.samples)
        ]
        print(
            json.dumps(
                {
                    "fixture": name,
                    "operation": command[0],
                    "samples": arguments.samples,
                    "median_wall_seconds": median([sample[0] for sample in samples]),
                    "median_user_seconds": median([sample[1] for sample in samples]),
                    "median_system_seconds": median([sample[2] for sample in samples]),
                    "binary": str(binary),
                },
                sort_keys=True,
                separators=(",", ":"),
            ),
            flush=True,
        )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)

