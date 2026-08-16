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
PHASES = {"waiting", "preparing", "ready", "active", "idle", "finishing", "finished", "failed", "retired"}


def now() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds")


def execution_root(repo: Path, exec_name: str) -> Path:
    validate_identifier(exec_name, "exec-name")
    return repo / "target" / "exp" / exec_name


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
