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
    parser.add_argument("--workloads", nargs="+",
                        choices=["constant", "functions", "types", "array", "forward-types", "repeated-family",
                                 "typed-types", "property-types", "shared-wide", "shared-deep"],
                        default=["constant", "functions", "types", "array"])
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
        cases[f"forward-types-{size}"] = "\n".join(
            f"type T{index} = T{index + 1};" for index in range(size - 1)
        ) + f"\ntype T{size - 1} = Int;\nexport def answer: Int = 42;\n"
        cases[f"repeated-family-{size}"] = "type Box(T) = struct {value: T};\n" + "\n".join(
            f"type T{index} = Box(Int);" for index in range(size)
        ) + "\nexport def answer: Int = 42;\n"
        property_prelude = (
            "@property(PropertyTarget.Type)\ntype Label = struct {text: String};\n"
            'def label: Fn(Type, Option(Label)) -> Label = fn(target, previous) { {text: "ready"} };\n'
        )
        for decorated in [False, True]:
            name = "property-types" if decorated else "typed-types"
            cases[f"{name}-{size}"] = property_prelude + "\n".join(
                ("@label\n" if decorated else "")
                + f"type T{index} = struct {{id: Int, name: String, values: Array(Int)}};\n"
                + f"def f{index}: Fn(T{index}) -> Int = fn(value) {{ value.id + 1 }};"
                for index in range(size)
            ) + "\nexport def ready: Bool = True;\n"
        shared_shapes = {
            "shared-wide": "struct {" + ", ".join(
                f"field{index}: Array(Int)" for index in range(32)
            ) + "}",
            "shared-deep": "struct {value: " + "Array(" * 32 + "Int" + ")" * 32 + "}",
        }
        for name, shape in shared_shapes.items():
            cases[f"{name}-{size}"] = (
                f"type Shared = {shape};\n"
                "def identity: for(T) Fn(T) -> T = fn(value) { value };\n"
                + "\n".join(
                    f"def f{index}: Fn(Shared) -> Shared = fn(value) {{ identity(value) }};"
                    for index in range(size)
                ) + "\nexport def ready: Bool = True;\n"
            )
    cases = {name: source for name, source in cases.items()
             if any(name == workload or name.startswith(workload + "-") for workload in args.workloads)}
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
