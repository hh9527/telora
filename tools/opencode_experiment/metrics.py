from __future__ import annotations

import fnmatch
import re
from pathlib import Path
from typing import Any, Callable


def _tokens(messages: list[dict[str, Any]]) -> dict[str, int]:
    result = {"input": 0, "output": 0, "reasoning": 0, "cache_read": 0, "cache_write": 0}
    for message in messages:
        values = message.get("info", {}).get("tokens", {})
        for name in ("input", "output", "reasoning"):
            result[name] += values.get(name, 0) or 0
        cache = values.get("cache", {})
        result["cache_read"] += cache.get("read", 0) or 0
        result["cache_write"] += cache.get("write", 0) or 0
    result["fresh"] = result["input"] + result["output"] + result["reasoning"]
    return result


def _time(messages: list[dict[str, Any]]) -> dict[str, int | None]:
    starts = [m.get("info", {}).get("time", {}).get("created") for m in messages]
    ends = [m.get("info", {}).get("time", {}).get("completed") for m in messages]
    starts = [value for value in starts if isinstance(value, (int, float))]
    ends = [value for value in ends if isinstance(value, (int, float))]
    active = sum(
        max(0, info["completed"] - info["created"] - _task_wait_ms(message))
        for message in messages
        if isinstance((info := message.get("info", {}).get("time", {})).get("created"), (int, float))
        and isinstance(info.get("completed"), (int, float))
    )
    first = min(starts) if starts else None
    last = max(ends) if ends else None
    span = last - first if first is not None and last is not None else None
    return {"first_created": first, "last_completed": last, "active_ms": active, "span_ms": span}


def _task_wait_ms(message: dict[str, Any]) -> int:
    info = message.get("info", {}).get("time", {})
    created = info.get("created")
    completed = info.get("completed")
    if not isinstance(created, (int, float)) or not isinstance(completed, (int, float)):
        return 0
    cursor = created
    waiting = 0
    for part in message.get("parts", []):
        if part.get("type") != "tool":
            continue
        state = part.get("state", {})
        end = state.get("time", {}).get("end")
        if not isinstance(end, (int, float)):
            continue
        command = state.get("input", {}).get("command", "")
        if (part.get("tool") == "bash" and isinstance(command, str)
                and re.search(r"(?:^|/)oc-task (?:pull|next) [a-z0-9._-]+(?:\s|$)", command)):
            waiting += max(0, min(end, completed) - cursor)
        cursor = max(cursor, end)
    return min(int(waiting), int(completed - created))


def _matches(path: str, patterns: list[str]) -> bool:
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def _write_paths(message: dict[str, Any]) -> list[str]:
    result = []
    for part in message.get("parts", []):
        if part.get("type") != "tool" or part.get("tool") not in ("write", "edit", "apply_patch"):
            continue
        values = part.get("state", {}).get("input", {})
        path = values.get("filePath") or values.get("file_path") or values.get("path")
        if isinstance(path, str): result.append(path)
    return result


def _relative_tool_path(path: str, workspace: Path) -> str | None:
    value = Path(path)
    if value.is_absolute():
        try: return value.relative_to(workspace).as_posix()
        except ValueError: return None
    return value.as_posix().removeprefix("./")


def _phase_messages(
    messages: list[dict[str, Any]], workspace: Path, definition: dict[str, Any] | None
) -> list[tuple[str, str, list[dict[str, Any]]]]:
    assistants = [message for message in messages if message.get("info", {}).get("role") == "assistant"]
    if definition is None:
        return [("unclassified", "unclassified", assistants)] if assistants else []

    learning_names = definition.get("learning_phases", [])
    work_phases = definition.get("work_phases") or [{
        "name": definition.get("work_phase", "work"),
        "files": definition.get("work_files", []),
    }]
    turn = -1
    assistant_turns: list[tuple[int, dict[str, Any]]] = []
    for message in messages:
        role = message.get("info", {}).get("role")
        if role == "user":
            turn += 1
        elif role == "assistant":
            assistant_turns.append((max(turn, 0), message))

    boundaries = []
    for order, phase in enumerate(work_phases):
        for index, (_turn, message) in enumerate(assistant_turns):
            paths = (_relative_tool_path(path, workspace) for path in _write_paths(message))
            if any(
                relative is not None and _matches(relative, phase["files"])
                for relative in paths
            ):
                boundaries.append((index, order, phase["name"]))
                break
    boundaries.sort()

    grouped: dict[tuple[str, str], list[dict[str, Any]]] = {}
    order: list[tuple[str, str]] = []
    for index, (message_turn, message) in enumerate(assistant_turns):
        active_work = [boundary for boundary in boundaries if boundary[0] <= index]
        if active_work:
            key = (active_work[-1][2], "work")
        else:
            name = learning_names[message_turn] if message_turn < len(learning_names) else f"learning_{message_turn + 1}"
            key = (name, "learning")
        if key not in grouped:
            grouped[key] = []
            order.append(key)
        grouped[key].append(message)
    return [(name, kind, grouped[(name, kind)]) for name, kind in order]


def _file_metric(path: Path) -> dict[str, int]:
    data = path.read_bytes()
    return {"files": 1, "lines": len(data.splitlines()), "bytes": len(data)}


def _add_metric(left: dict[str, int], right: dict[str, int]) -> dict[str, int]:
    return {name: left.get(name, 0) + right.get(name, 0) for name in ("files", "lines", "bytes")}


def _artifact_metrics(workspace: Path, definition: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for kind, categories in definition.get("artifacts", {}).items():
        kind_files: set[Path] = set()
        category_values = {}
        for category, patterns in categories.items():
            paths: set[Path] = set()
            for pattern in patterns:
                paths.update(path for path in workspace.glob(pattern) if path.is_file() and not path.is_symlink())
            metric = {"files": 0, "lines": 0, "bytes": 0}
            for path in sorted(paths):
                metric = _add_metric(metric, _file_metric(path))
            category_values[category] = metric
            kind_files.update(paths)
        total = {"files": 0, "lines": 0, "bytes": 0}
        for path in sorted(kind_files):
            total = _add_metric(total, _file_metric(path))
        result[kind] = {"categories": category_values, "total": total}
    return result


def _sum_values(values: list[dict[str, int]], names: tuple[str, ...]) -> dict[str, int]:
    return {name: sum(value.get(name, 0) for value in values) for name in names}


def collect_metrics(
    exec_name: str,
    execution_phase: str,
    workspace: Path,
    children: list[dict[str, Any]],
    load_messages: Callable[[str], list[dict[str, Any]]],
    config: dict[str, Any],
) -> dict[str, Any]:
    configured = config.get("roles", {})
    roles = []
    for child in children:
        agent = child.get("agent")
        session_id = child.get("id") or child.get("session_id")
        if not isinstance(agent, str) or not isinstance(session_id, str):
            continue
        messages = load_messages(session_id)
        definition = configured.get(agent)
        phases = []
        for name, kind, phase_messages in _phase_messages(messages, workspace, definition):
            phases.append({"name": name, "kind": kind, "messages": len(phase_messages),
                           "time": _time(phase_messages), "tokens": _tokens(phase_messages)})
        assistant = [m for m in messages if m.get("info", {}).get("role") == "assistant"]
        time = _time(assistant)
        elapsed = time["span_ms"]
        time["waiting_ms"] = max(0, elapsed - time["active_ms"]) if elapsed is not None else None
        model = child.get("model", {})
        artifacts = _artifact_metrics(workspace, definition or {})
        work_fresh = sum(phase["tokens"]["fresh"] for phase in phases if phase["kind"] == "work")
        code_lines = artifacts.get("code", {}).get("total", {}).get("lines", 0)
        roles.append({
            "agent": agent,
            "session_id": session_id,
            "title": child.get("title"),
            "model": {"provider": model.get("providerID"), "id": model.get("id"), "variant": model.get("variant")},
            "classification": {
                "configured": definition is not None,
                "work_boundary_observed": any(phase["kind"] == "work" for phase in phases) if definition else None,
            },
            "time": time,
            "tokens": _tokens(assistant),
            "phases": phases,
            "artifacts": artifacts,
            "productivity": {
                "code_lines_per_1k_work_fresh_tokens": round(code_lines * 1000 / work_fresh, 3) if work_fresh else None,
            },
        })
    roles.sort(key=lambda item: item["agent"])

    token_names = ("input", "output", "reasoning", "cache_read", "cache_write", "fresh")
    token_total = _sum_values([role["tokens"] for role in roles], token_names)
    phase_totals = {}
    for kind in ("learning", "work", "unclassified"):
        selected = [phase for role in roles for phase in role["phases"] if phase["kind"] == kind]
        phase_totals[kind] = {
            "active_ms": sum(phase["time"]["active_ms"] for phase in selected),
            "tokens": _sum_values([phase["tokens"] for phase in selected], token_names),
        }
    artifact_totals = {}
    for kind in ("code", "documents"):
        values = [role["artifacts"][kind]["total"] for role in roles if kind in role["artifacts"]]
        artifact_totals[kind] = _sum_values(values, ("files", "lines", "bytes"))
    starts = [role["time"]["first_created"] for role in roles if role["time"]["first_created"] is not None]
    ends = [role["time"]["last_completed"] for role in roles if role["time"]["last_completed"] is not None]
    first = min(starts) if starts else None
    last = max(ends) if ends else None
    return {
        "schema": "telora.opencode-stats/v1",
        "exec_name": exec_name,
        "execution_phase": execution_phase,
        "roles": roles,
        "aggregate": {
            "time": {
                "first_created": first,
                "last_completed": last,
                "span_ms": last - first if first is not None and last is not None else None,
            },
            "active_ms": sum(role["time"]["active_ms"] for role in roles),
            "tokens": token_total,
            "phases": phase_totals,
            "artifacts": artifact_totals,
            "productivity": {
                "code_lines_per_1k_work_fresh_tokens": round(
                    artifact_totals["code"]["lines"] * 1000 / phase_totals["work"]["tokens"]["fresh"], 3
                ) if phase_totals["work"]["tokens"]["fresh"] else None,
            },
        },
    }
