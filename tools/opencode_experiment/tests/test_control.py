from __future__ import annotations

import json
import os
import subprocess
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock

from tools.opencode_experiment.client import Client
from tools.opencode_experiment.config import ControlError, load_manifest, safe_relative, validate_identifier
from tools.opencode_experiment.observe import failures, latest_assistant, normalized, summarize
from tools.opencode_experiment.query import select_engine
from tools.opencode_experiment.external import probe_direct, probe_mise, resolve_capabilities, resolve_cli, resolve_command
from tools.opencode_experiment.state import atomic_json, bind_plan, load_state, save_state, SCHEMA
from tools.opencode_experiment.lifecycle import copy_archive, export_session, prepare
from tools.opencode_experiment.context import Context


class Handler(BaseHTTPRequestHandler):
    messages: list[dict] = []

    def log_message(self, *_args): pass

    def response(self, value, code=200):
        body = json.dumps(value).encode(); self.send_response(code); self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/global/health"): self.response({"healthy": True})
        elif self.path.startswith("/session/status"): self.response({"ses_test": {"type": "idle"}})
        elif "/message?" in self.path: self.response(self.messages)
        elif self.path.startswith("/broken"): self.send_response(200); self.end_headers(); self.wfile.write(b"not-json")
        else: self.response({}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0)); payload = json.loads(self.rfile.read(length) or b"{}")
        if self.path.startswith("/session?"): self.response({"id": "ses_test"})
        elif "/prompt_async?" in self.path:
            text = payload["parts"][0]["text"]
            self.messages.append({"info": {"id": f"usr_{len(self.messages)}", "role": "user", "time": {"created": 1}}, "parts": [{"type": "text", "text": text}]})
            self.response(None)
        else: self.response({}, 404)


class ServerTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        Handler.messages = []; cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler); cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True); cls.thread.start()
        cls.client = Client(f"http://127.0.0.1:{cls.server.server_port}", "/tmp/ws", "ses_test")

    @classmethod
    def tearDownClass(cls): cls.server.shutdown(); cls.server.server_close(); cls.thread.join()

    def test_contract(self):
        self.assertTrue(self.client.health()["healthy"]); self.assertEqual(self.client.status()["type"], "idle")
        self.assertEqual(Client(self.client.url, "/tmp/ws").create_session("test")["id"], "ses_test")
        self.client.prompt("hello"); self.assertEqual(self.client.messages()[-1]["parts"][0]["text"], "hello")

    def test_loopback_only(self):
        with self.assertRaises(ControlError): Client("http://example.com:12", "/tmp/ws")

    def test_unavailable(self):
        with self.assertRaises(ControlError): Client("http://127.0.0.1:1", "/tmp/ws", timeout=.01).health()


class ConfigStateTest(unittest.TestCase):
    def test_identifiers_and_paths(self):
        self.assertEqual(validate_identifier("a2-001", "exec"), "a2-001")
        for value in ("../x", "/x", ".", "a/../b"):
            with self.assertRaises(ControlError): safe_relative(value)
        for value in ("A", "a/b", ".hidden", "a b"):
            with self.assertRaises(ControlError): validate_identifier(value, "id")

    def test_binding_and_atomic_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); root = bind_plan(repo, "plan", "run")
            self.assertEqual((root / "plan").read_text(), "plan\n")
            with self.assertRaises(ControlError): bind_plan(repo, "other", "run")
            state = {"schema": SCHEMA, "plan_id": "plan", "exec_name": "run", "phase": "ready"}; save_state(root, state)
            self.assertEqual(load_state(root), state)

    def test_manifest_unknown_key(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); plan = root / "experiments" / "p"; plan.mkdir(parents=True)
            (plan / "experiment.json").write_text(json.dumps({"schema": "telora.opencode-experiment/v1", "typo": 1}))
            with self.assertRaisesRegex(ControlError, "unknown manifest"): load_manifest(root, "p")

    def test_manifest_requires_exact_prompts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); plan = root / "experiments" / "p"; plan.mkdir(parents=True)
            base = {
                "schema": "telora.opencode-experiment/v1",
                "prompts": {"start": "Start.", "continue": "Continue.", "feedback": "Feedback."},
                "workspace": {"template": "template", "copies": []},
                "permissions": {"read": ["output/**"], "write": ["output/**"], "commands": ["./bin/run"]},
                "feedback": {"path": "output/feedback.md", "role_writable": False},
            }
            for prompts in (
                {"start": "Start.", "feedback": "Feedback."},
                {"start": "Start.", "continue": "", "feedback": "Feedback."},
                {"start": "Start.", "cont": "Continue.", "feedback": "Feedback."},
            ):
                (plan / "experiment.json").write_text(json.dumps({**base, "prompts": prompts}))
                with self.assertRaises(ControlError): load_manifest(root, "p")


class ObserveQueryTest(unittest.TestCase):
    def setUp(self):
        self.messages = [
            {"info": {"id": "u", "role": "user", "time": {"created": 10}}, "parts": [{"type": "text", "text": "go"}]},
            {"info": {"id": "a", "role": "assistant", "finish": "stop", "time": {"created": 11, "completed": 20}, "tokens": {"reasoning": 3}}, "parts": [{"type": "tool", "tool": "bash", "state": {"status": "error", "input": {}, "metadata": {"exit": 1}, "output": "bad"}}, {"type": "text", "text": "done"}]},
        ]

    def test_summary_and_failures(self):
        summary = summarize(self.messages); self.assertEqual(summary["duration_ms"], 10); self.assertEqual(summary["tool_failures"], 1)
        self.assertEqual(failures(self.messages)[0]["count"], 1); self.assertEqual(latest_assistant(self.messages)["info"]["id"], "a")

    def test_query_selection(self):
        with mock.patch.dict(os.environ, {"OC_QUERY_ENGINE": "jq"}), mock.patch("tools.opencode_experiment.query.probe_direct", return_value=("jq",)):
            self.assertEqual(select_engine(), ("jq", ["jq"]))
        with mock.patch.dict(os.environ, {"OC_QUERY_ENGINE": "bad"}):
            with self.assertRaises(ControlError): select_engine()

    def test_cli_mise_fallback(self):
        resolve_cli.cache_clear(); probe_direct.cache_clear(); probe_mise.cache_clear()

    def test_manifest_command_uses_mise_prefix(self):
        with mock.patch("tools.opencode_experiment.external.resolve_cli", return_value=("mise", "x", "--", "cargo")):
            self.assertEqual(
                resolve_command(["cargo", "build", "-p", "demo"]),
                ["mise", "x", "--", "cargo", "build", "-p", "demo"],
            )
        failed = subprocess.CompletedProcess([], 127, "", "missing")
        passed = subprocess.CompletedProcess([], 0, "1.0", "")
        with mock.patch("subprocess.run", side_effect=[failed, passed]):
            self.assertEqual(resolve_cli("opencode"), ("mise", "x", "--", "opencode"))
        resolve_cli.cache_clear(); probe_direct.cache_clear(); probe_mise.cache_clear()

    def test_query_prefers_direct_jq_over_mise_jaq(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("tools.opencode_experiment.external.probe_direct", side_effect=lambda cli: (cli,) if cli == "jq" else None), mock.patch("tools.opencode_experiment.external.probe_mise", return_value=None):
            self.assertEqual(select_engine(), ("jq", ["jq"]))


class MigrationTest(unittest.TestCase):
    def manifest(self, root: Path) -> None:
        plan = root / "experiments" / "demo"; (plan / "template" / "a2").mkdir(parents=True)
        (plan / "template" / "a2" / "feedback.md").write_text("")
        (plan / "input.md").write_text("input")
        (root / "bin").mkdir(); artifact = root / "bin" / "tool"; artifact.write_text("tool")
        (plan / "experiment.json").write_text(json.dumps({
            "schema": "telora.opencode-experiment/v1", "prompts": {"start": "Start.", "continue": "Continue.", "feedback": "Read feedback."},
            "workspace": {"template": "template", "copies": [{"from": "input.md", "to": "a1/input.md", "mode": "0444"}]},
            "permissions": {"read": ["a1/**", "a2/**"], "write": ["a2/src/**"], "commands": ["./bin/run"]},
            "feedback": {"path": "a2/feedback.md", "role_writable": False},
            "artifacts": [{"name": "tool", "source": "bin/tool", "to": "bin/tool", "mode": "0555"}],
            "validation": [], "observe": ["a2"], "archive": ["a1", "a2", "opencode.json"]
        }))

    def test_prepare_and_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); self.manifest(repo)
            with mock.patch("tools.opencode_experiment.lifecycle.repository_root", return_value=repo), mock.patch("tools.opencode_experiment.lifecycle.git_metadata", return_value=("rev", False)):
                root, state, created = prepare("demo", "run-1", 4567)
                _, resumed, created_again = prepare("demo", "run-1", None)
            self.assertTrue(created); workspace = Path(state["workspace"])
            self.assertFalse(created_again); self.assertEqual(resumed["server_url"], "http://127.0.0.1:4567")
            self.assertEqual((workspace / "a1/input.md").read_text(), "input")
            self.assertEqual((workspace / "a1/input.md").stat().st_mode & 0o777, 0o444)
            self.assertEqual((workspace / "bin/tool").stat().st_mode & 0o777, 0o555)
            config = json.loads((workspace / "opencode.json").read_text())
            self.assertEqual(config["permission"]["write"]["a2/feedback.md"], "deny")
            context = Context(repo, root, state, load_manifest(repo, "demo")); destination = root / "result" / "workspace"
            copy_archive(context, destination); self.assertTrue((destination / "a1/input.md").is_file())
            copy_archive(context, destination); self.assertTrue((destination / "a1/input.md").is_file())
            os.symlink("/tmp", workspace / "a2" / "escape")
            context.manifest = type(context.manifest)(**{**context.manifest.__dict__, "archive": ("a2",)})
            with self.assertRaises(ControlError): copy_archive(context, destination)

    def test_export_retries_truncated_json(self):
        context = mock.Mock()
        context.state = {"workspace": "/tmp/ws"}
        payloads = iter((b'{"messages":["', b'{"messages":[]}'))
        def run(*_args, **kwargs):
            kwargs["stdout"].write(next(payloads))
            return subprocess.CompletedProcess([], 0, b"", b"")
        with mock.patch("tools.opencode_experiment.lifecycle.resolve_cli", return_value=("opencode",)), \
             mock.patch("tools.opencode_experiment.lifecycle.subprocess.run", side_effect=run) as run_mock, \
             mock.patch("tools.opencode_experiment.lifecycle.time.sleep"):
            self.assertEqual(export_session(context, "ses_test"), {"messages": []})
            self.assertEqual(run_mock.call_count, 2)

    def test_export_reports_failure_after_three_attempts(self):
        context = mock.Mock()
        context.state = {"workspace": "/tmp/ws"}
        failed = subprocess.CompletedProcess([], 1, b"", b"bad export")
        with mock.patch("tools.opencode_experiment.lifecycle.resolve_cli", return_value=("opencode",)), \
             mock.patch("tools.opencode_experiment.lifecycle.subprocess.run", return_value=failed) as run, \
             mock.patch("tools.opencode_experiment.lifecycle.time.sleep"):
            with self.assertRaisesRegex(ControlError, "bad export"):
                export_session(context, "ses_test")
            self.assertEqual(run.call_count, 3)


if __name__ == "__main__": unittest.main()
