from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .client import Client
from .config import ControlError, Manifest, load_manifest, repository_root, validate_identifier
from .state import execution_root, load_state


@dataclass
class Context:
    repo: Path
    root: Path
    state: dict[str, Any]
    manifest: Manifest

    def client(self) -> Client:
        return Client(self.state["server_url"], self.state["workspace"], self.state["session_id"])

    def rounds(self) -> list[dict[str, Any]]:
        result = []
        directory = self.root / "rounds"
        if directory.exists():
            for path in sorted(directory.glob("*.json")):
                try: result.append(json.loads(path.read_text(encoding="utf-8")))
                except (OSError, json.JSONDecodeError): raise ControlError(f"invalid round record: {path}") from None
        return result


def resolve(exec_name: str, cwd: Path | None = None) -> Context:
    validate_identifier(exec_name, "exec-name")
    repo = repository_root(cwd); root = execution_root(repo, exec_name); state = load_state(root)
    if state.get("exec_name") != exec_name: raise ControlError("execution name mismatch")
    return Context(repo, root, state, load_manifest(repo, state["plan_id"]))
