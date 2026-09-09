#!/usr/bin/env python3
"""Compare module-analysis costs using identical generated source inputs."""

import argparse
import json
from pathlib import Path
import statistics
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", nargs="+", type=Path)
    parser.add_argument("--sizes", nargs="+", type=int, default=[100, 400])
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--save-workspace", type=Path,
                        help="Copy the generated workspace to a new directory for separate profiling")
    parser.add_argument("--workloads", nargs="+",
                        choices=["constant", "functions", "types", "array", "forward-types", "repeated-family",
                                 "typed-types", "property-types", "checked-types", "recursive-types", "shared-wide", "shared-deep",
                                 "family-contracts", "qualified-family-contracts",
                                 "recursive-families",
                                 "family-obligations", "qualified-family-obligations",
                                 "property-constraints", "qualified-property-constraints",
                                 "module-fanout", "module-diamond"],
                        default=["constant", "functions", "types", "array"])
    args = parser.parse_args()
    if args.samples < 1 or args.timeout <= 0 or any(size < 1 for size in args.sizes):
        parser.error("sizes, samples and timeout must be positive")
    binaries = [binary.resolve(strict=True) for binary in args.binaries]
    if args.save_workspace is not None and args.save_workspace.exists():
        parser.error("save-workspace must name a new directory")
    cases = {"constant": "export def answer: Int = 42;\n"}
    dependencies = {}
    for size in args.sizes:
        cases[f"functions-{size}"] = "\n".join(
            f"def f{index}: Fn(Int) -> Int = fn(x) {{ x + 1 }};"
            for index in range(size)
        ) + "\nexport def answer: Int = f0(1);\n"
        cases[f"types-{size}"] = "\n".join(
            f"type T{index} = struct {{value: Int}};" for index in range(size)
        ) + "\nexport def answer: Int = 42;\n"
        cases[f"checked-types-{size}"] = "\n".join(
            '@check(fn(value) { if value > 0 { Ok(()) } '
            'else { Err(blame!("expected positive", value)) } })\n'
            + f"type T{index} = struct(Int);"
            for index in range(size)
        ) + "\nexport def answer: Int = 42;\n"
        cases[f"recursive-types-{size}"] = "\n".join(
            f"type T{index} = struct {{children: Array(T{index})}};"
            for index in range(size)
        ) + "\nexport def answer: Int = 42;\n"
        cases[f"recursive-families-{size}"] = "\n".join(
            f"type Tree{index}(T) = struct {{value: T, children: Array(Tree{index}(T))}};"
            + f"\ntype IntTree{index} = Tree{index}(Int);"
            for index in range(size)
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
        for qualified in [False, True]:
            name = f"{'qualified-' if qualified else ''}property-constraints-{size}"
            declaration = "type Label = struct {text: String};\n"
            if qualified:
                dependencies[f"{name}-types"] = "export " + declaration
                prelude = f'import "./{name}-types" as model;\n'
                property_type = "model.Label"
            else:
                prelude, property_type = declaration, "Label"
            cases[name] = prelude + "\n".join(
                f"def f{index}: for(T: Property({property_type})) Fn(T) -> T = fn(value) {{ value }};"
                for index in range(size)
            ) + "\nexport def ready: Bool = True;\n"
        for qualified in [False, True]:
            name = f"{'qualified-' if qualified else ''}family-obligations-{size}"
            declarations = "export type Label = struct {text: String};\nexport type Box(T: Property(Label)) = Array(T);\n"
            if qualified:
                dependencies[f"{name}-types"] = declarations
                prelude = f'import "./{name}-types" as model;\n'
                label, family = "model.Label", "model.Box"
            else:
                prelude, label, family = declarations, "Label", "Box"
            cases[name] = prelude + "\n".join(
                f"def f{index}: for(T: Property({label})) Fn({family}(T)) -> {family}(T) = fn(value) {{ value }};"
                for index in range(size)
            ) + "\nexport def ready: Bool = True;\n"
        for qualified in [False, True]:
            name = f"{'qualified-' if qualified else ''}family-contracts-{size}"
            declaration = "type Box(T) = struct {value: T, items: Array(T)};\n"
            if qualified:
                dependencies[f"{name}-types"] = "export " + declaration
                prelude = f'import "./{name}-types" as model;\n'
                family = "model.Box"
            else:
                prelude, family = declaration, "Box"
            cases[name] = prelude + "\n".join(
                f"def f{index}: Fn({family}(Int)) -> {family}(Int) = fn(value) {{ value }};"
                for index in range(size)
            ) + "\nexport def ready: Bool = True;\n"
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
        for workload in ["module-fanout", "module-diamond"]:
            root = f"{workload}-{size}"
            if workload == "module-diamond":
                dependencies[f"{root}-shared"] = (
                    "export type Item = struct {value: Int};\n"
                    "export def value: Item = {value: 42};\n"
                )
            for index in range(size):
                dependencies[f"{root}-arm{index}"] = (
                    f'import "./{root}-shared" {{Item, value}};\nexport {{Item, value}};\n'
                    if workload == "module-diamond"
                    else f"export def value: Int = {index};\n"
                )
            cases[root] = "\n".join(
                f'import "./{root}-arm{index}" as m{index};' for index in range(size)
            ) + "\nexport def answer: Int = " + (
                "m0.value.value" if workload == "module-diamond" else "m0.value"
            ) + ";\n"
    cases = {name: source for name, source in cases.items()
             if any(name == workload or name.startswith(workload + "-") for workload in args.workloads)}
    dependencies = {name: source for name, source in dependencies.items()
                    if any(name.startswith(root + "-") for root in cases)}
    modules = {**cases, **dependencies}
    with tempfile.TemporaryDirectory(prefix="telora-inference-") as directory:
        workspace = Path(directory)
        (workspace / "src").mkdir()
        (workspace / "telora-config.json").write_text(
            json.dumps({"version": 1, "members": ["."]}), encoding="ascii"
        )
        (workspace / "telora-crate.json").write_text(
            json.dumps({
                "name": "inference-bench",
                "modules": [f"@src/{name}" for name in modules],
                "dependencies": [],
            }), encoding="ascii"
        )
        for name, source in modules.items():
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
        if args.save_workspace is not None:
            shutil.copytree(workspace, args.save_workspace)


if __name__ == "__main__":
    main()
