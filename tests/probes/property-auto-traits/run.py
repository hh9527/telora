#!/usr/bin/env python3
"""在临时副本中验收表达能力，并记录尚未解决的语言边界。"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--telora", required=True, type=Path)
args = parser.parse_args()
compiler = args.telora.resolve()
source = Path(__file__).resolve().parent
temporary = Path(os.environ.get("TMPDIR") or Path.home() / ".cache/tmp")
temporary.mkdir(parents=True, exist_ok=True)
output = Path(tempfile.mkdtemp(prefix="property-auto-traits-", dir=temporary))


def workspace(name):
    directory = output / name
    shutil.copytree(source / "model", directory / "model")
    shutil.copy2(source / "telora-config.json", directory / "telora-config.json")
    locked = subprocess.run(
        [str(compiler), "-C", str(directory), "lock"],
        text=True, capture_output=True, timeout=30,
    )
    assert locked.returncode == 0, locked.stderr
    return directory


def evaluate(directory, name):
    result = subprocess.run(
        [str(compiler), "-C", str(directory / "model"), "eval", f"property_auto_traits:{name}"],
        text=True, capture_output=True, timeout=120,
    )
    (directory / f"{name}.stdout").write_text(result.stdout)
    (directory / f"{name}.stderr").write_text(result.stderr)
    return result


positive = workspace("positive")
expected = [
    {"id": "devices", "table": "device_rows", "fields": ["id", "name"], "description": "Managed devices"},
    {"id": "alarms", "table": "alarm_rows", "fields": ["device", "id"], "description": None},
    {"id": "manual", "table": "manual_rows", "fields": ["value"], "description": None},
]
result = evaluate(positive, "result")
assert result.returncode == 0, result.stderr
assert json.loads(result.stdout) == expected, result.stdout
result = evaluate(positive, "tables")
assert result.returncode == 0, result.stderr
assert json.loads(result.stdout) == ["device_rows", "alarm_rows", "manual_rows"]

directory = workspace("optional-present")
root = directory / "model/src/lib.telora"
root.write_text(root.read_text().replace(
    '@model::dataset("alarms", "alarm_rows")',
    '@model::dataset("alarms", "alarm_rows")\n@model::description("Alarm stream")',
))
result = evaluate(directory, "result")
assert result.returncode == 0, result.stderr
with_note = [dict(item) for item in expected]
with_note[1]["description"] = "Alarm stream"
assert json.loads(result.stdout) == with_note, result.stdout

observations = []
for case, export, diagnostic in [
    ("missing-dataset", "rejected", "Missing does not implement Dataset"),
    ("erased-type", "result", "Type metadata does not establish a specific TypeOf witness"),
    ("field-property", "rejected", "Record does not satisfy Property(Column)"),
]:
    directory = workspace(case)
    root = directory / "model/src/lib.telora"
    root.write_text(root.read_text() + "\n" + (source / f"cases/{case}.telora").read_text())
    result = evaluate(directory, export)
    assert result.returncode != 0, f"{case} unexpectedly accepted"
    assert diagnostic in result.stderr, result.stderr
    observations.append({"case": case, "status": "static_rejection", "diagnostic": result.stderr})

directory = workspace("unused-provider")
shutil.copy2(source / "cases/unused.telora", directory / "model/src/unused.telora")
root = directory / "model/src/lib.telora"
root.write_text(root.read_text() + "\npub mod unused;\n")
result = evaluate(directory, "result")
assert result.returncode != 0, "Update this probe: unused provider no longer initializes"
assert "unused Property provider was initialized" in result.stderr, result.stderr
observations.append({"case": "unused-provider", "status": "observed_retention_blocker", "diagnostic": result.stderr})

report = {"workspace": str(output), "positive": "passed", "optional_property_added": "passed", "observations": observations}
(output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(report, ensure_ascii=False, indent=2))
