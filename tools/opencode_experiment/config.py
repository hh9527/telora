from __future__ import annotations

import hashlib
import json
import re
import subprocess
from dataclasses import dataclass, field
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
    permission_preflight: dict[str, tuple[str, ...]] = field(default_factory=dict)
    reporting: dict[str, Any] = field(default_factory=lambda: {"sinks": []})
    manifest_name: str = "experiment.json"
    metrics: dict[str, Any] = field(default_factory=lambda: {"roles": {}})
    workflow: dict[str, Any] | None = None

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
    _keys(data, {"schema", "prompts", "validation", "archive", "observe", "artifacts", "environment", "permission_preflight", "reporting", "metrics", "workflow"}, "manifest")
    if data.get("schema") != "telora.opencode-cloned-plan/v1":
        raise ControlError("unsupported experiment manifest schema")

    prompts = data.get("prompts")
    if not isinstance(prompts, dict) or set(prompts) != {"start", "continue"} or not all(isinstance(x, str) and x.strip() for x in prompts.values()):
        raise ControlError("prompts must contain nonempty start and continue strings")
    validation = data.get("validation", [])
    artifacts = data.get("artifacts", [])
    environment = data.get("environment", {})
    permission_preflight = data.get("permission_preflight", {})
    reporting = data.get("reporting", {"sinks": []})
    metrics = data.get("metrics", {"roles": {}})
    workflow = data.get("workflow")
    archive = _string_array(data.get("archive", []), "archive")
    observe = _string_array(data.get("observe", []), "observe")
    if not isinstance(validation, list) or not isinstance(artifacts, list):
        raise ControlError("validation and artifacts must be arrays")
    if not isinstance(environment, dict):
        raise ControlError("environment must be an object")
    if not isinstance(permission_preflight, dict):
        raise ControlError("permission_preflight must be an object")
    normalized_preflight = {}
    for role, commands in permission_preflight.items():
        validate_identifier(role, "permission preflight role")
        normalized_preflight[role] = tuple(_string_array(commands, f"permission_preflight.{role}"))
    if not isinstance(reporting, dict):
        raise ControlError("reporting must be an object")
    _keys(reporting, {"sinks"}, "reporting")
    sinks = reporting.get("sinks", [])
    if not isinstance(sinks, list):
        raise ControlError("reporting.sinks must be an array")
    normalized_sinks = []
    for sink in sinks:
        if not isinstance(sink, dict):
            raise ControlError("reporting sink must be an object")
        _keys(sink, {"kind", "repository", "issue"}, "reporting sink")
        if sink.get("kind") != "github_issue_comment":
            raise ControlError(f"unsupported reporting sink: {sink.get('kind')!r}")
        repository = sink.get("repository")
        issue = sink.get("issue")
        if not isinstance(repository, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise ControlError("invalid GitHub reporting repository")
        if not isinstance(issue, int) or issue <= 0:
            raise ControlError("invalid GitHub reporting issue")
        normalized_sinks.append(dict(sink))
    if not isinstance(metrics, dict):
        raise ControlError("metrics must be an object")
    _keys(metrics, {"roles"}, "metrics")
    metric_roles = metrics.get("roles", {})
    if not isinstance(metric_roles, dict):
        raise ControlError("metrics.roles must be an object")
    normalized_metric_roles: dict[str, Any] = {}
    for role, definition in metric_roles.items():
        validate_identifier(role, "metrics role")
        if not isinstance(definition, dict):
            raise ControlError(f"metrics.roles.{role} must be an object")
        _keys(definition, {"learning_phases", "work_phase", "work_files", "artifacts"}, f"metrics.roles.{role}")
        learning_phases = _string_array(definition.get("learning_phases", []), f"metrics.roles.{role}.learning_phases")
        for phase in learning_phases:
            validate_identifier(phase, "metrics learning phase")
        work_phase = definition.get("work_phase", "work")
        if not isinstance(work_phase, str):
            raise ControlError(f"metrics.roles.{role}.work_phase must be a string")
        validate_identifier(work_phase, "metrics work phase")
        work_files = _string_array(definition.get("work_files", []), f"metrics.roles.{role}.work_files")
        for pattern in work_files:
            safe_relative(pattern, "metrics work file pattern")
        artifact_kinds = definition.get("artifacts", {})
        if not isinstance(artifact_kinds, dict):
            raise ControlError(f"metrics.roles.{role}.artifacts must be an object")
        _keys(artifact_kinds, {"code", "documents"}, f"metrics.roles.{role}.artifacts")
        normalized_artifacts: dict[str, dict[str, list[str]]] = {}
        for kind, categories in artifact_kinds.items():
            if not isinstance(categories, dict):
                raise ControlError(f"metrics.roles.{role}.artifacts.{kind} must be an object")
            normalized_categories = {}
            for category, patterns in categories.items():
                validate_identifier(category, "metrics artifact category")
                values = _string_array(patterns, f"metrics.roles.{role}.artifacts.{kind}.{category}")
                for pattern in values:
                    safe_relative(pattern, "metrics artifact pattern")
                normalized_categories[category] = values
            normalized_artifacts[kind] = normalized_categories
        normalized_metric_roles[role] = {
            "learning_phases": learning_phases,
            "work_phase": work_phase,
            "work_files": work_files,
            "artifacts": normalized_artifacts,
        }
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
    if workflow is not None:
        from .task_cli import TaskError, validate_workflow
        try:
            workflow = validate_workflow(workflow)
        except TaskError as exc:
            raise ControlError(str(exc), exc.code) from None
    return Manifest(plan_id, root, dict(prompts), tuple(validation), tuple(archive),
                    tuple(observe), tuple(artifacts), dict(environment), normalized_preflight,
                    {"sinks": normalized_sinks}, metrics={"roles": normalized_metric_roles},
                    workflow=workflow)
