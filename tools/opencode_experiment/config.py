from __future__ import annotations

import hashlib
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any


class ControlError(Exception):
    def __init__(self, message: str, code: int = 65):
        super().__init__(message)
        self.code = code


IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")
OPENCODE_ENVIRONMENT = {
    "OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": lambda value: value.isascii() and value.isdigit() and int(value) > 0,
}


def validate_identifier(value: str, kind: str) -> str:
    if not IDENTIFIER.fullmatch(value) or value.startswith(".") or ".." in value.split("."):
        raise ControlError(f"invalid {kind}: {value!r}", 64)
    return value


def safe_relative(value: str, kind: str = "path") -> PurePosixPath:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or any(part in ("", ".", "..") for part in value.split("/")):
        raise ControlError(f"unsafe {kind}: {value!r}")
    return path


def repository_root(cwd: Path | None = None) -> Path:
    from .external import resolve_cli
    result = subprocess.run(
        [*resolve_cli("git"), "rev-parse", "--show-toplevel"], cwd=cwd, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if result.returncode:
        raise ControlError("current directory is not inside a Git worktree", 66)
    return Path(result.stdout.strip()).resolve()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _keys(value: dict[str, Any], allowed: set[str], where: str) -> None:
    unknown = set(value) - allowed
    if unknown:
        raise ControlError(f"unknown {where} key(s): {', '.join(sorted(unknown))}")


@dataclass(frozen=True)
class Manifest:
    plan_id: str
    root: Path
    prompts: dict[str, str]
    validation: tuple[dict[str, Any], ...]
    archive: tuple[str, ...]
    observe: tuple[str, ...]
    artifacts: tuple[dict[str, Any], ...]
    environment: dict[str, str]
    manifest_name: str = "experiment.json"

def _string_array(value: Any, where: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ControlError(f"{where} must be a string array")
    return value


def load_manifest(repo: Path, plan_id: str) -> Manifest:
    validate_identifier(plan_id, "plan-id")
    root = repo / "experiments" / plan_id
    path = root / "experiment.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ControlError(f"missing experiment plan manifest: {path}", 66) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise ControlError(f"invalid experiment plan manifest: {exc}") from None
    if not isinstance(data, dict):
        raise ControlError("experiment manifest must be an object")
    _keys(data, {"schema", "prompts", "validation", "archive", "observe", "artifacts", "environment"}, "manifest")
    if data.get("schema") != "telora.opencode-cloned-plan/v1":
        raise ControlError("unsupported experiment manifest schema")

    prompts = data.get("prompts")
    if not isinstance(prompts, dict) or set(prompts) != {"start", "continue"} or not all(isinstance(x, str) and x.strip() for x in prompts.values()):
        raise ControlError("prompts must contain nonempty start and continue strings")
    validation = data.get("validation", [])
    artifacts = data.get("artifacts", [])
    environment = data.get("environment", {})
    archive = _string_array(data.get("archive", []), "archive")
    observe = _string_array(data.get("observe", []), "observe")
    if not isinstance(validation, list) or not isinstance(artifacts, list):
        raise ControlError("validation and artifacts must be arrays")
    if not isinstance(environment, dict):
        raise ControlError("environment must be an object")
    for name, value in environment.items():
        validator = OPENCODE_ENVIRONMENT.get(name)
        if validator is None:
            raise ControlError(f"unsupported opencode environment variable: {name}")
        if not isinstance(value, str) or not validator(value):
            raise ControlError(f"invalid value for opencode environment variable: {name}")
    for item in validation:
        if not isinstance(item, dict):
            raise ControlError("validation entry must be an object")
        _keys(item, {"name", "command", "cwd", "required"}, "validation")
        validate_identifier(str(item.get("name", "")), "validation name")
        if not _string_array(item.get("command"), "validation command"):
            raise ControlError("validation command must be nonempty")
        if "cwd" in item:
            safe_relative(str(item["cwd"]), "validation cwd")
        if not isinstance(item.get("required", True), bool):
            raise ControlError("validation.required must be boolean")
    for item in artifacts:
        if not isinstance(item, dict):
            raise ControlError("artifact entry must be an object")
        _keys(item, {"name", "source", "to", "build", "mode"}, "artifact")
        validate_identifier(str(item.get("name", "")), "artifact name")
        safe_relative(str(item.get("source", "")), "artifact source")
        safe_relative(str(item.get("to", "")), "artifact destination")
        if "build" in item and not _string_array(item["build"], "artifact build"):
            raise ControlError("artifact build must be nonempty")
    for item in (*archive, *observe):
        safe_relative(item)
    return Manifest(plan_id, root, dict(prompts), tuple(validation), tuple(archive),
                    tuple(observe), tuple(artifacts), dict(environment))
