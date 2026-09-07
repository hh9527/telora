#!/usr/bin/env python3
"""Compare module-analysis costs using identical generated source inputs."""

import argparse
import json
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", nargs="+", type=Path)
    parser.add_argument("--sizes", nargs="+", type=int, default=[100, 400])
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if args.samples < 1 or args.timeout <= 0 or any(size < 1 for size in args.sizes):
        parser.error("sizes, samples and timeout must be positive")
    binaries = [binary.resolve(strict=True) for binary in args.binaries]
    cases = {"constant": "export def answer: Int = 42;\n"}
    for size in args.sizes:
        cases[f"functions-{size}"] = "\n".join(
            f"def f{index}: Fn(Int) -> Int = fn(x) {{ x + 1 }};"
            for index in range(size)
        ) + "\nexport def answer: Int = f0(1);\n"
        cases[f"types-{size}"] = "\n".join(
            f"type T{index} = struct {{value: Int}};" for index in range(size)
        ) + "\nexport def answer: Int = 42;\n"
        cases[f"array-{size}"] = (
            "export def values: Array(Int) = ["
            + ",".join(str(index) for index in range(size))
            + "];\n"
        )
    with tempfile.TemporaryDirectory(prefix="telora-inference-") as directory:
        workspace = Path(directory)
        (workspace / "src").mkdir()
        (workspace / "telora-config.json").write_text(
            json.dumps({"version": 1, "members": ["."]}), encoding="ascii"
        )
        (workspace / "telora-crate.json").write_text(
            json.dumps({
                "name": "inference-bench",
                "modules": [f"@src/{name}" for name in cases],
                "dependencies": [],
            }), encoding="ascii"
        )
        for name, source in cases.items():
            (workspace / "src" / f"{name}.telora").write_text(source, encoding="ascii")
        for binary in binaries:
            command = [str(binary), "-C", directory]
            subprocess.run(command + ["lock"], check=True, capture_output=True, timeout=args.timeout)
            for name in cases:
                samples = []
                for sample in range(args.samples + 1):
                    start = time.perf_counter()
                    result = subprocess.run(
                        command + ["check", f"@src/{name}"],
                        capture_output=True, timeout=args.timeout,
                    )
                    elapsed = time.perf_counter() - start
                    if result.returncode:
                        raise RuntimeError(f"{binary}: {name}: {result.stderr.decode()}")
                    if sample:
                        samples.append(elapsed)
                print(json.dumps({
                    "binary": str(binary), "case": name,
                    "median_seconds": statistics.median(samples), "samples_seconds": samples,
                }), flush=True)


if __name__ == "__main__":
    main()
