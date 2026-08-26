from __future__ import annotations

import fcntl
import json
import os
import tempfile
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator

from .config import ControlError, validate_identifier

SCHEMA = "telora.opencode-execution/v1"
RUN_CONFIG_SCHEMA = "telora.opencode-run-config/v1"
RUNNER_CONFIG_SCHEMA = "telora.opencode-runner-config/v1"
CONNECT_TEST_SCHEMA = "telora.opencode-connect-test/v1"
PHASES = {"waiting", "preparing", "ready", "active", "idle", "finishing", "finished", "failed", "retired"}


def now() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def execution_root(repo: Path, exec_name: str) -> Path:
    validate_identifier(exec_name, "exec-name")
    return repo / "target" / "exp" / exec_name


def run_config_path(repo: Path, test_id: str) -> Path:
    return execution_root(repo, test_id) / "config.json"


def runner_config_path(repo: Path, test_id: str) -> Path:
    return execution_root(repo, test_id) / "runner.json"


def runner_workspace_path(repo: Path, test_id: str) -> Path:
    return execution_root(repo, test_id) / "runner-workspace"


def create_runner_config(repo: Path, test_id: str, port: int) -> dict[str, Any]:
    validate_identifier(test_id, "test-id")
    if not 1 <= port <= 65535:
        raise ControlError("port must be from 1 through 65535", 64)
    root = execution_root(repo, test_id)
    with locked(root):
        path = runner_config_path(repo, test_id)
        if path.is_file():
            value = load_runner_config(repo, test_id)
            if value["port"] != port:
                raise ControlError(f"runner {test_id} is already configured for another port")
            return value
        value = {
            "schema": RUNNER_CONFIG_SCHEMA,
            "test_id": test_id,
            "port": port,
            "created_at": now(),
        }
        atomic_json(path, value)
        return value


def load_runner_config(repo: Path, test_id: str) -> dict[str, Any]:
    path = runner_config_path(repo, test_id)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ControlError(
            f"missing external runner for {test_id}; run oc-run {test_id} <port> before start",
            75,
        ) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise ControlError(f"invalid runner configuration: {exc}") from None
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "test_id", "port", "created_at"}
        or value.get("schema") != RUNNER_CONFIG_SCHEMA
        or value.get("test_id") != test_id
        or not isinstance(value.get("port"), int)
        or not 1 <= value["port"] <= 65535
        or not isinstance(value.get("created_at"), str)
    ):
        raise ControlError("invalid runner configuration")
    return value


def connect_test_path(repo: Path, test_id: str) -> Path:
    return execution_root(repo, test_id) / "connect-test.json"


def record_connect_test(repo: Path, test_id: str, result: dict[str, Any]) -> dict[str, Any]:
    validate_identifier(test_id, "test-id")
    value = {
        "schema": CONNECT_TEST_SCHEMA,
        "test_id": test_id,
        "tested_at": now(),
        "transport": "opencode-loopback-http",
        "health": result.get("health"),
        "session_id": result.get("session_id"),
    }
    if value["health"] is not True:
        raise ControlError("connection test did not report a healthy daemon")
    if not isinstance(value["session_id"], str) or not value["session_id"].startswith("ses_"):
        raise ControlError("connection test did not create a valid session")
    atomic_json(connect_test_path(repo, test_id), value)
    return value


def load_connect_test(repo: Path, test_id: str) -> dict[str, Any]:
    path = connect_test_path(repo, test_id)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ControlError(
            f"missing connection test for {test_id}; run oc-ctl test-connect {test_id} before start",
            75,
        ) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise ControlError(f"invalid connection test: {exc}") from None
    if (
        not isinstance(value, dict)
        or set(value)
        != {"schema", "test_id", "tested_at", "transport", "health", "session_id"}
        or value.get("schema") != CONNECT_TEST_SCHEMA
        or value.get("test_id") != test_id
        or value.get("transport") != "opencode-loopback-http"
        or value.get("health") is not True
        or not isinstance(value.get("tested_at"), str)
        or not isinstance(value.get("session_id"), str)
        or not value["session_id"].startswith("ses_")
    ):
        raise ControlError("invalid connection test receipt")
    return value


def _validate_run_config(value: Any, test_id: str) -> dict[str, Any]:
    required = {"schema", "test_id", "plan_id", "port", "created_at"}
    if (not isinstance(value, dict) or not required.issubset(value)
            or not set(value).issubset(required | {"from_test_id", "bundle"})):
        raise ControlError("invalid run configuration")
    if value.get("schema") != RUN_CONFIG_SCHEMA or value.get("test_id") != test_id:
        raise ControlError("run configuration identity mismatch")
    if not isinstance(value.get("plan_id"), str):
        raise ControlError("invalid run configuration plan-id")
    validate_identifier(value["plan_id"], "plan-id")
    if not isinstance(value.get("port"), int) or not 1 <= value["port"] <= 65535:
        raise ControlError("invalid run configuration port")
    if "from_test_id" in value:
        source = value["from_test_id"]
        if not isinstance(source, str):
            raise ControlError("invalid source test-id")
        validate_identifier(source, "source test-id")
        if source == test_id:
            raise ControlError("an execution cannot inherit from itself")
    if "bundle" in value and (not isinstance(value["bundle"], str)
                              or not Path(value["bundle"]).is_absolute()):
        raise ControlError("invalid bundle path")
    return value


def load_run_config(repo: Path, test_id: str) -> dict[str, Any]:
    path = run_config_path(repo, test_id)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ControlError(f"missing run configuration: {path}", 66) from None
    except (OSError, json.JSONDecodeError) as exc:
        raise ControlError(f"invalid run configuration: {exc}") from None
    return _validate_run_config(value, test_id)


def create_run_config(repo: Path, test_id: str, plan_id: str, port: int,
                      from_test_id: str | None = None,
                      bundle: str | None = None) -> dict[str, Any]:
    validate_identifier(test_id, "test-id")
    validate_identifier(plan_id, "plan-id")
    if from_test_id is not None:
        validate_identifier(from_test_id, "source test-id")
        if from_test_id == test_id:
            raise ControlError("an execution cannot inherit from itself", 64)
    if not 1 <= port <= 65535:
        raise ControlError("port must be from 1 through 65535", 64)
    bundle_path = str(Path(bundle).expanduser().resolve()) if bundle is not None else None
    root = execution_root(repo, test_id)
    with locked(root):
        path = run_config_path(repo, test_id)
        if path.is_file():
            try:
                value = _validate_run_config(json.loads(path.read_text(encoding="utf-8")), test_id)
            except (OSError, json.JSONDecodeError) as exc:
                raise ControlError(f"invalid run configuration: {exc}") from None
            if value["plan_id"] != plan_id:
                raise ControlError(f"execution {test_id} is already configured for {value['plan_id']}")
            if value.get("from_test_id") != from_test_id:
                raise ControlError("execution is already configured with another source", 64)
            if value.get("bundle") != bundle_path:
                raise ControlError("execution is already configured with another bundle", 64)
            return value
        value = {"schema": RUN_CONFIG_SCHEMA, "test_id": test_id, "plan_id": plan_id,
                 "port": port, "created_at": now()}
        if from_test_id is not None:
            value["from_test_id"] = from_test_id
        if bundle_path is not None:
            value["bundle"] = bundle_path
        atomic_json(path, value)
        return value


def bind_plan(repo: Path, plan_id: str, exec_name: str) -> Path:
    root = execution_root(repo, exec_name); root.mkdir(parents=True, exist_ok=True)
    binding = root / "plan"; expected = f"{plan_id}\n"
    if binding.exists():
        if binding.read_text(encoding="utf-8") != expected:
            raise ControlError(f"execution {exec_name} is bound to another plan")
    else:
        atomic_write(binding, expected.encode(), 0o444)
    return root

def atomic_write(path: Path, content: bytes, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as output:
            output.write(content); output.flush(); os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def atomic_json(path: Path, data: Any) -> None:
    atomic_write(path, (json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode())


@contextmanager
def locked(root: Path, exclusive: bool = True) -> Iterator[None]:
    root.mkdir(parents=True, exist_ok=True)
    with (root / "lock").open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
        try: yield
        finally: fcntl.flock(lock, fcntl.LOCK_UN)

def load_state(root: Path) -> dict[str, Any]:
    try: data = json.loads((root / "state.json").read_text(encoding="utf-8"))
    except FileNotFoundError: raise ControlError(f"missing execution state: {root}", 66) from None
    except (OSError, json.JSONDecodeError) as exc: raise ControlError(f"invalid execution state: {exc}") from None
    if not isinstance(data, dict) or data.get("schema") != SCHEMA:
        raise ControlError("unsupported execution state schema")
    if data.get("phase") not in PHASES: raise ControlError("invalid execution phase")
    binding = (root / "plan").read_text(encoding="utf-8")
    if binding != f"{data.get('plan_id')}\n": raise ControlError("execution plan identity mismatch")
    return data


def save_state(root: Path, state: dict[str, Any]) -> None:
    atomic_json(root / "state.json", state)
