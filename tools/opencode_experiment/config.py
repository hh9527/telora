from __future__ import annotations

import hashlib
import json
import re
import shlex
import subprocess
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any


class ControlError(Exception):
    def __init__(self, message: str, code: int = 65):
        super().__init__(message)
        self.code = code


IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")
TELORA_SUBCOMMANDS = frozenset({"check", "lsp", "query", "run"})


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
    workspace: tuple[str, ...]
    roles: dict[str, dict[str, Any]]
    validation: tuple[dict[str, Any], ...]
    archive: tuple[str, ...]
    observe: tuple[str, ...]
    artifacts: tuple[dict[str, Any], ...]
    reporting: dict[str, Any] = field(default_factory=lambda: {"sinks": []})
    manifest_name: str = "experiment.json"
    metrics: dict[str, Any] = field(default_factory=lambda: {"roles": {}})
    workflow: dict[str, Any] | None = None

    @property
    def permission_preflight(self) -> dict[str, tuple[str, ...]]:
        return {name: tuple(role["preflight"]) for name, role in self.roles.items()}

def _string_array(value: Any, where: str) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ControlError(f"{where} must be a string array")
    return value


def _validate_permission_preflight_command(command: str, where: str) -> None:
    try:
        tokens = shlex.split(command)
    except ValueError as exc:
        raise ControlError(f"invalid {where} command: {exc}") from None
    if not tokens:
        raise ControlError(f"{where} command must not be empty")
    if PurePosixPath(tokens[0]).name != "telora":
        return
    subcommand = tokens[1] if len(tokens) > 1 else None
    if subcommand not in TELORA_SUBCOMMANDS:
        expected = ", ".join(sorted(TELORA_SUBCOMMANDS))
        raise ControlError(
            f"unsupported Telora subcommand in {where}: {subcommand!r}; "
            f"expected one of {expected}"
        )


def load_manifest(repo: Path, plan_id: str) -> Manifest:
    validate_identifier(plan_id, "plan-id")
    root = repo / "experiment-plans" / plan_id
    path = root / "experiment.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ControlError(f"missing experiment plan manifest: {path}", 66) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise ControlError(f"invalid experiment plan manifest: {exc}") from None
    if not isinstance(data, dict):
        raise ControlError("experiment manifest must be an object")
    _keys(data, {"schema", "workspace", "roles", "validation", "archive", "observe", "artifacts", "reporting", "metrics", "workflow"}, "manifest")
    if data.get("schema") != "telora.experiment-plan/v1":
        raise ControlError("unsupported experiment manifest schema")

    workspace = _string_array(data.get("workspace", []), "workspace")
    for item in workspace:
        safe_relative(item, "workspace path")
        source = root / item
        if not source.exists() or source.is_symlink():
            raise ControlError(f"missing or unsafe workspace input: {item}", 66)
    roles = data.get("roles")
    if not isinstance(roles, dict) or not roles:
        raise ControlError("roles must be a non-empty object")
    normalized_roles: dict[str, dict[str, Any]] = {}
    for name, role in roles.items():
        validate_identifier(name, "role")
        if not isinstance(role, dict):
            raise ControlError(f"roles.{name} must be an object")
        _keys(role, {"description", "instructions", "read", "write", "commands", "preflight"}, f"roles.{name}")
        description = role.get("description")
        instructions = role.get("instructions")
        if not isinstance(description, str) or not description.strip():
            raise ControlError(f"roles.{name}.description must be nonempty")
        if not isinstance(instructions, str):
            raise ControlError(f"roles.{name}.instructions must be a path")
        safe_relative(instructions, f"roles.{name}.instructions")
        if not (root / instructions).is_file():
            raise ControlError(f"missing role instructions: {instructions}", 66)
        normalized_role = {"description": description, "instructions": instructions}
        for key in ("read", "write", "commands", "preflight"):
            values = _string_array(role.get(key, []), f"roles.{name}.{key}")
            if key in ("read", "write"):
                for value in values:
                    safe_relative(value.removesuffix("/**").removesuffix("/*"), f"roles.{name}.{key}")
            if key == "preflight":
                for index, command in enumerate(values):
                    _validate_permission_preflight_command(command, f"roles.{name}.preflight[{index}]")
            normalized_role[key] = values
        normalized_roles[name] = normalized_role
    validation = data.get("validation", [])
    artifacts = data.get("artifacts", [])
    reporting = data.get("reporting", {"sinks": []})
    metrics = data.get("metrics", {"roles": {}})
    workflow = data.get("workflow")
    archive = _string_array(data.get("archive", []), "archive")
    observe = _string_array(data.get("observe", []), "observe")
    if not isinstance(validation, list) or not isinstance(artifacts, list):
        raise ControlError("validation and artifacts must be arrays")
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
        _keys(definition, {"learning_phases", "work_phase", "work_files", "work_phases", "artifacts", "commands"}, f"metrics.roles.{role}")
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
        work_phases = definition.get("work_phases")
        if work_phases is not None and ("work_phase" in definition or "work_files" in definition):
            raise ControlError(
                f"metrics.roles.{role} cannot combine work_phases with work_phase/work_files"
            )
        normalized_work_phases = []
        if work_phases is not None:
            if not isinstance(work_phases, list) or not work_phases:
                raise ControlError(f"metrics.roles.{role}.work_phases must be a non-empty array")
            seen_work_phases = set()
            for index, phase in enumerate(work_phases):
                context = f"metrics.roles.{role}.work_phases[{index}]"
                if not isinstance(phase, dict):
                    raise ControlError(f"{context} must be an object")
                _keys(phase, {"name", "files"}, context)
                name = phase.get("name")
                if not isinstance(name, str):
                    raise ControlError(f"{context}.name must be a string")
                validate_identifier(name, "metrics work phase")
                if name in seen_work_phases:
                    raise ControlError(f"duplicate metrics work phase: {name}")
                seen_work_phases.add(name)
                files = _string_array(phase.get("files", []), f"{context}.files")
                if not files:
                    raise ControlError(f"{context}.files must not be empty")
                for pattern in files:
                    safe_relative(pattern, "metrics work file pattern")
                normalized_work_phases.append({"name": name, "files": files})
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
        command_kinds = definition.get("commands", {})
        if not isinstance(command_kinds, dict):
            raise ControlError(f"metrics.roles.{role}.commands must be an object")
        normalized_commands: dict[str, list[str]] = {}
        for name, patterns in command_kinds.items():
            validate_identifier(name, "metrics command")
            values = _string_array(patterns, f"metrics.roles.{role}.commands.{name}")
            if not values:
                raise ControlError(f"metrics.roles.{role}.commands.{name} must not be empty")
            normalized_commands[name] = values
        normalized_definition = {
            "learning_phases": learning_phases,
            "artifacts": normalized_artifacts,
            "commands": normalized_commands,
        }
        if normalized_work_phases:
            normalized_definition["work_phases"] = normalized_work_phases
        else:
            normalized_definition["work_phase"] = work_phase
            normalized_definition["work_files"] = work_files
        normalized_metric_roles[role] = normalized_definition
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
        if list(normalized_roles) != workflow["roles"]:
            raise ControlError("manifest roles must match workflow roles in declaration order")
    unknown_metric_roles = set(normalized_metric_roles) - set(normalized_roles)
    if unknown_metric_roles:
        raise ControlError(f"metrics reference unknown role(s): {', '.join(sorted(unknown_metric_roles))}")
    return Manifest(plan_id, root, tuple(workspace), normalized_roles, tuple(validation),
                    tuple(archive), tuple(observe), tuple(artifacts),
                    {"sinks": normalized_sinks}, metrics={"roles": normalized_metric_roles},
                    workflow=workflow)
