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


def validate_identifier(value: str, kind: str) -> str:
    if not IDENTIFIER.fullmatch(value) or value.startswith(".") or ".." in value.split("."):
        raise ControlError(f"invalid {kind}: {value!r}", 64)
    return value


def safe_relative(value: str, kind: str = "path") -> PurePosixPath:
    raw_parts = value.split("/")
    path = PurePosixPath(value)
    if not value or path.is_absolute() or any(part in ("", ".", "..") for part in raw_parts):
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


def _keys(obj: dict[str, Any], allowed: set[str], where: str) -> None:
    unknown = set(obj) - allowed
    if unknown:
        raise ControlError(f"unknown {where} key(s): {', '.join(sorted(unknown))}")


@dataclass(frozen=True)
class Manifest:
    plan_id: str
    root: Path
    prompts: dict[str, str]
    template: str
    copies: tuple[dict[str, Any], ...]
    permissions: dict[str, Any]
    feedback: dict[str, Any]
    validation: tuple[dict[str, Any], ...]
    archive: tuple[str, ...]
    observe: tuple[str, ...]
    artifacts: tuple[dict[str, Any], ...]

    def source(self, relative: str) -> Path:
        candidate = (self.root / safe_relative(relative)).resolve()
        if not candidate.is_relative_to(self.root.resolve()):
            raise ControlError(f"plan path escapes plan root: {relative}")
        return candidate


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
    _keys(data, {"schema", "prompts", "workspace", "permissions", "feedback", "validation", "archive", "observe", "artifacts"}, "manifest")
    if data.get("schema") != "telora.opencode-experiment/v1":
        raise ControlError("unsupported experiment manifest schema")
    workspace = data.get("workspace")
    prompts = data.get("prompts")
    permissions = data.get("permissions")
    feedback = data.get("feedback")
    if not isinstance(prompts, dict) or not isinstance(workspace, dict) or not isinstance(permissions, dict) or not isinstance(feedback, dict):
        raise ControlError("prompts, workspace, permissions, and feedback must be objects")
    _keys(prompts, {"start", "continue", "feedback"}, "prompts")
    if set(prompts) != {"start", "continue", "feedback"} or not all(isinstance(value, str) and value.strip() for value in prompts.values()):
        raise ControlError("prompts must contain nonempty start, continue, and feedback strings")
    _keys(workspace, {"template", "copies"}, "workspace")
    _keys(permissions, {"read", "write", "commands"}, "permissions")
    _keys(feedback, {"path", "role_writable"}, "feedback")
    copies = workspace.get("copies", [])
    if not isinstance(copies, list):
        raise ControlError("workspace.copies must be an array")
    destinations: set[str] = set()
    for copy in copies:
        if not isinstance(copy, dict):
            raise ControlError("workspace copy must be an object")
        _keys(copy, {"from", "to", "mode"}, "workspace copy")
        safe_relative(str(copy.get("from", "")), "copy source")
        target = str(safe_relative(str(copy.get("to", "")), "copy destination"))
        if target in destinations:
            raise ControlError(f"duplicate copy destination: {target}")
        destinations.add(target)
        if not re.fullmatch(r"0[0-7]{3}", str(copy.get("mode", ""))):
            raise ControlError(f"invalid copy mode for {target}")
    template = str(safe_relative(str(workspace.get("template", "")), "workspace template"))
    for key in ("read", "write", "commands"):
        if not isinstance(permissions.get(key), list) or not all(isinstance(x, str) and x for x in permissions[key]):
            raise ControlError(f"permissions.{key} must be a string array")
    for key in ("read", "write"):
        for pattern in permissions[key]:
            if pattern.startswith("/") or any(part in ("", ".", "..") for part in pattern.split("/")):
                raise ControlError(f"unsafe permissions.{key} pattern: {pattern!r}")
    for command in permissions["commands"]:
        if not command.startswith("./") or any(part in ("", ".", "..") for part in command[2:].split("/")):
            raise ControlError(f"unsafe permission command: {command!r}")
    safe_relative(str(feedback.get("path", "")), "feedback path")
    if feedback.get("role_writable") is not False:
        raise ControlError("v1 feedback path must be Host-owned")
    validation = data.get("validation", [])
    artifacts = data.get("artifacts", [])
    archive = data.get("archive", [])
    observe = data.get("observe", [])
    if not isinstance(validation, list) or not isinstance(artifacts, list) or not isinstance(archive, list) or not isinstance(observe, list):
        raise ControlError("validation, artifacts, archive, and observe must be arrays")
    for item in validation:
        if not isinstance(item, dict): raise ControlError("validation entry must be an object")
        _keys(item, {"name", "command", "required"}, "validation")
        if not isinstance(item.get("command"), list) or not all(isinstance(x, str) and x for x in item["command"]):
            raise ControlError("validation command must be a nonempty argument array")
        if not isinstance(item.get("name"), str) or not IDENTIFIER.fullmatch(item["name"]):
            raise ControlError("validation name must be an identifier")
    for item in artifacts:
        if not isinstance(item, dict): raise ControlError("artifact entry must be an object")
        _keys(item, {"name", "source", "to", "build", "mode"}, "artifact")
        safe_relative(str(item.get("source", "")), "artifact source")
        safe_relative(str(item.get("to", "")), "artifact destination")
        if "build" in item and (not isinstance(item["build"], list) or not all(isinstance(x, str) and x for x in item["build"])):
            raise ControlError("artifact build must be a nonempty argument array")
    for item in archive: safe_relative(str(item), "archive path")
    for item in observe: safe_relative(str(item), "observe path")
    return Manifest(plan_id, root, dict(prompts), template,
                    tuple(copies), permissions, feedback, tuple(validation), tuple(str(x) for x in archive),
                    tuple(str(x) for x in observe), tuple(artifacts))
