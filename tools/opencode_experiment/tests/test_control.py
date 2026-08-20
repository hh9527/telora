from __future__ import annotations

import json
import os
import subprocess
import tempfile
import threading
import unittest
from contextlib import redirect_stdout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import StringIO
from pathlib import Path
from unittest import mock

from tools.opencode_experiment.client import Client
from tools.opencode_experiment.config import ControlError, Manifest, load_manifest, safe_relative, validate_identifier
from tools.opencode_experiment.observe import failures, latest_assistant, normalized, summarize
from tools.opencode_experiment.query import select_engine
from tools.opencode_experiment.external import probe_direct, probe_mise, resolve_capabilities, resolve_cli, resolve_command
from tools.opencode_experiment.state import atomic_json, create_run_config, load_run_config, load_state, save_state, SCHEMA
from tools.opencode_experiment.lifecycle import copy_archive, export_session, opencode_environment, prepare, request_start, reserve, run_validation, start_requested
from tools.opencode_experiment.metrics import collect_metrics
from tools.opencode_experiment.context import Context
from tools.opencode_experiment.permissions import preflight_permissions
from tools.opencode_experiment.reporting import submit_report
from tools.opencode_experiment.watch import WatchWindow, acp_events, message_events, watch_progress
from tools.opencode_experiment.cli_ctl import _configure_start, _selected_plan, _update, main as control_main, parser as control_parser
from tools.opencode_experiment.cli_run import parser as run_parser


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
        self.client.prompt_session("ses_test", "continue"); self.assertEqual(self.client.messages()[-1]["parts"][0]["text"], "continue")

    def test_loopback_only(self):
        with self.assertRaises(ControlError): Client("http://example.com:12", "/tmp/ws")

    def test_unavailable(self):
        with self.assertRaises(ControlError): Client("http://127.0.0.1:1", "/tmp/ws", timeout=.01).health()


class ConfigStateTest(unittest.TestCase):
    @staticmethod
    def write_plan(plan: Path, *, environment: dict[str, str] | None = None) -> None:
        plan.mkdir(parents=True)
        (plan / "experiment.json").write_text(json.dumps({
            "schema": "telora.opencode-cloned-plan/v1",
            "prompts": {"start": "start", "continue": "continue"},
            "artifacts": [{"name": "tool", "source": "tool", "to": "bin/tool", "mode": "0555"}],
            "validation": [], "observe": ["bin"], "archive": ["bin", "opencode.json", "experiment.json"],
            "environment": environment or {},
        }))
        (plan / "opencode.json").write_text(json.dumps({"default_agent": "main", "permission": "deny"}))

    def test_identifiers_and_paths(self):
        self.assertEqual(validate_identifier("a2-001", "exec"), "a2-001")
        for value in ("../x", "/x", ".", "a/../b"):
            with self.assertRaises(ControlError): safe_relative(value)
        for value in ("A", "a/b", ".hidden", "a b"):
            with self.assertRaises(ControlError): validate_identifier(value, "id")

    def test_manifest_rejects_unknown_telora_preflight_subcommand(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiments" / "demo"
            self.write_plan(plan)
            path = plan / "experiment.json"
            manifest = json.loads(path.read_text(encoding="utf-8"))
            manifest["permission_preflight"] = {
                "a1": ["./bin/telora types --limit 20"],
            }
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                ControlError,
                r"unsupported Telora subcommand.*'types'.*check, lsp, query, run",
            ):
                load_manifest(repo, "demo")

            manifest["permission_preflight"]["a1"] = [
                "./bin/telora query exports @bin/main.telora -C query-builder",
            ]
            path.write_text(json.dumps(manifest), encoding="utf-8")
            self.assertEqual(
                load_manifest(repo, "demo").permission_preflight["a1"],
                ("./bin/telora query exports @bin/main.telora -C query-builder",),
            )

    def test_artifact_publication_command_is_available(self):
        args = control_parser().parse_args(["publish", "run", "draft", "result"])
        self.assertEqual((args.command, args.test_id, args.artifacts),
                         ("publish", "run", ["draft", "result"]))

    def test_stat_command_is_available(self):
        args = control_parser().parse_args(["stat", "run"])
        self.assertEqual((args.command, args.test_id), ("stat", "run"))

    def test_control_surface_is_limited_to_five_commands(self):
        self.assertEqual(set(control_parser()._subparsers._group_actions[0].choices),
                         {"start", "stat", "status", "update", "publish"})

    def test_run_uses_only_test_id(self):
        args = run_parser().parse_args(["ontology-3-006"])
        self.assertEqual(vars(args), {"test_id": "ontology-3-006"})

    def test_host_selects_plan_and_writes_run_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiments" / "demo"
            self.write_plan(plan)
            self.assertEqual(_selected_plan(repo, plan / "nested"), "demo")
            with mock.patch("tools.opencode_experiment.cli_ctl.Path.cwd", return_value=plan), \
                 mock.patch("tools.opencode_experiment.cli_ctl._automatic_port", return_value=43123):
                value = _configure_start(repo, "run-001")
            self.assertEqual(value["plan_id"], "demo")
            self.assertEqual(value["port"], 43123)
            self.assertEqual(load_run_config(repo, "run-001"), value)
            self.assertEqual(create_run_config(repo, "run-001", "demo", 49999), value)

    def test_update_copies_and_removes_workspace_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            host = root / "host"
            workspace = root / "workspace"
            host.mkdir()
            workspace.mkdir()
            (host / "feedback.md").write_text("revise", encoding="utf-8")
            context = mock.Mock(state={"phase": "idle", "workspace": str(workspace)})
            with mock.patch("tools.opencode_experiment.cli_ctl.Path.cwd", return_value=host):
                _update(context, ["docs/FEEDBACK.md=feedback.md"])
                self.assertEqual((workspace / "docs/FEEDBACK.md").read_text(), "revise")
                _update(context, ["docs/FEEDBACK.md=!"])
                self.assertFalse((workspace / "docs/FEEDBACK.md").exists())

    def test_update_accepts_absolute_host_source_outside_repository(self):
        with tempfile.TemporaryDirectory(dir="/tmp") as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            source = root / "host-input.md"
            workspace.mkdir()
            source.write_text("external input", encoding="utf-8")
            context = mock.Mock(state={"phase": "idle", "workspace": str(workspace)})

            result = _update(context, [f"docs/INPUT.md={source}"])

            self.assertEqual((workspace / "docs/INPUT.md").read_text(), "external input")
            self.assertEqual(result[0]["source"], str(source))

    def test_update_preserves_source_mode(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            source = root / "tool"
            workspace.mkdir()
            source.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            source.chmod(0o775)
            context = mock.Mock(state={"phase": "idle", "workspace": str(workspace)})

            result = _update(context, [f"bin/tool={source}"])

            destination = workspace / "bin/tool"
            self.assertEqual(destination.stat().st_mode & 0o7777, 0o775)
            self.assertEqual(result[0]["mode"], "0775")

    def test_update_still_rejects_unsafe_destination(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            source = root / "input.md"
            workspace.mkdir()
            source.write_text("input", encoding="utf-8")
            context = mock.Mock(state={"phase": "idle", "workspace": str(workspace)})

            with self.assertRaisesRegex(ControlError, "unsafe destination"):
                _update(context, [f"../escaped.md={source}"])
            self.assertFalse((root / "escaped.md").exists())

    def test_atomic_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); (root / "plan").write_text("plan\n")
            state = {"schema": SCHEMA, "plan_id": "plan", "exec_name": "run", "phase": "ready"}; save_state(root, state)
            self.assertEqual(load_state(root), state)

    def test_ontology_3_pins_model_and_uses_file_driven_workflow(self):
        repo = Path(__file__).resolve().parents[3]
        plan = repo / "experiments" / "ontology-3"
        model = "deepseek/deepseek-v4-flash"
        self.assertEqual(json.loads((plan / "opencode.json").read_text())["model"], model)
        for role in ("coordinator", "a1", "a2", "a3", "a4"):
            text = (plan / ".opencode" / "agents" / f"{role}.md").read_text(encoding="utf-8")
            self.assertIn(f'model: "{model}"', text)
        coordinator = (plan / ".opencode" / "agents" / "coordinator.md").read_text(encoding="utf-8")
        self.assertIn("同时启动 A1、A2、A3、A4 各一次", coordinator)
        self.assertNotIn("touch", coordinator)
        manifest = load_manifest(repo, "ontology-3")
        self.assertEqual(
            [phase["name"] for phase in manifest.metrics["roles"]["a3"]["work_phases"]],
            ["modeling", "query_surface_design"],
        )
        workflow = manifest.workflow
        self.assertEqual(workflow["schema"], "telora.opencode-artifact-workflow/v1")
        self.assertEqual(workflow["start_artifacts"],
                         ["lang", "qb-req", "edsl-req", "domain-ent-1", "intent-req"])
        self.assertEqual(workflow["finish_artifact"], "intent-1")
        artifacts = workflow["artifacts"]
        self.assertEqual(artifacts["qb.a1"]["owner"], "a1")
        self.assertEqual(artifacts["edsl.a2"]["owner"], "a2")
        self.assertEqual(artifacts["ent-1-model.a3"]["owner"], "a3")
        self.assertEqual(artifacts["intent-1.a4"]["owner"], "a4")
        self.assertIsNone(artifacts["qb"]["owner"])
        self.assertEqual(artifacts["qb.a1"]["input"], [
            {"id": "lang", "optional": False},
            {"id": "qb-req", "optional": False},
            {"id": "qb-feedback", "optional": True},
        ])
        self.assertEqual(artifacts["qb-feedback.a2"]["owner"], "a2")
        self.assertEqual(artifacts["qb-feedback.a3"]["owner"], "a3")
        self.assertIsNone(artifacts["qb-feedback"]["owner"])
        self.assertEqual(next(item for item in manifest.artifacts if item["name"] == "telora")["source"],
                         "target/release/telora")
        tutorial = next(item for item in manifest.artifacts if item["name"] == "lang-tutorial")
        self.assertEqual((tutorial["source"], tutorial["to"]),
                         ("guide/TELORA.md", "docs/TELORA.md"))
        cli_guide = next(item for item in manifest.artifacts if item["name"] == "cli-guide")
        self.assertEqual((cli_guide["source"], cli_guide["to"]),
                         ("guide/TELORA-CLI.md", "docs/TELORA-CLI.md"))
        self.assertIn("./bin/oc-task pull a1", manifest.permission_preflight["a1"])
        self.assertIn("./bin/oc-task submit a2 *", manifest.permission_preflight["a2"])
        self.assertIn("./bin/oc-task submit a4 *", manifest.permission_preflight["a4"])
        a4 = (plan / ".opencode" / "agents" / "a4.md").read_text(encoding="utf-8")
        a4_permission_line = next(
            line.removeprefix("permission: ")
            for line in a4.splitlines()
            if line.startswith("permission: ")
        )
        a4_permissions = json.loads(a4_permission_line)
        self.assertEqual(a4_permissions["read"]["*"], "deny")
        self.assertNotIn("ent-1/**", a4_permissions["read"])
        self.assertNotIn("./bin/telora query at * -C intent-1", a4_permissions["bash"])
        self.assertEqual(
            a4_permissions["bash"]["./bin/telora query exports @bin/main.telora -C intent-1"],
            "allow",
        )
        self.assertFalse(any("mark-blocked" in command for commands in manifest.permission_preflight.values()
                             for command in commands))
        self.assertEqual((plan / "ontology" / "QUERY-BUILDER-FEEDBACK.md").stat().st_size, 0)
        self.assertEqual((plan / "ent-1" / "QUERY-BUILDER-FEEDBACK.md").stat().st_size, 0)
        self.assertEqual((plan / "intent-1" / "FEEDBACK.md").stat().st_size, 0)
        domain = (plan / "ent-1" / "DOMAIN.md").read_text(encoding="utf-8")
        ontology_goal = (plan / "ontology" / "GOAL.md").read_text(encoding="utf-8")
        self.assertNotIn("一次结果必须同时保留", domain)
        self.assertNotIn("多个非法意图产生诊断", ontology_goal)

    def test_manifest_validates_opencode_environment(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiments" / "demo"
            self.write_plan(plan, environment={"OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": "128000"})
            manifest = load_manifest(repo, "demo")
            self.assertEqual(manifest.environment, {"OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": "128000"})
            self.assertEqual(opencode_environment({"opencode_environment": manifest.environment})["OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX"], "128000")

        for environment in ({"PATH": "/tmp"}, {"OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": "0"},
                            {"OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX": "lots"}):
            with tempfile.TemporaryDirectory() as temporary:
                repo = Path(temporary); plan = repo / "experiments" / "demo"
                self.write_plan(plan, environment=environment)
                with self.assertRaises(ControlError):
                    load_manifest(repo, "demo")

    def test_manifest_validates_metrics(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiments" / "demo"
            self.write_plan(plan)
            path = plan / "experiment.json"
            data = json.loads(path.read_text())
            data["metrics"] = {"roles": {"worker": {
                "learning_phases": ["language_learning"],
                "work_phase": "implementation",
                "work_files": ["output/src/main.telora"],
                "artifacts": {
                    "code": {"core": ["output/src/*.telora"]},
                    "documents": {"docs": ["output/NOTES.md"]},
                },
            }}}
            path.write_text(json.dumps(data))
            metrics = load_manifest(repo, "demo").metrics
            self.assertEqual(metrics["roles"]["worker"]["work_phase"], "implementation")
            self.assertEqual(metrics["roles"]["worker"]["artifacts"]["code"]["core"], ["output/src/*.telora"])

    def test_prepare_clones_committed_plan_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiments" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            subprocess.run(["git", "init", "--quiet"], cwd=plan, check=True)
            subprocess.run(["git", "add", "."], cwd=plan, check=True)
            subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--quiet", "-m", "plan"], cwd=plan, check=True)
            git = ("git",)
            with mock.patch("tools.opencode_experiment.lifecycle.repository_root", return_value=repo), \
                 mock.patch("tools.opencode_experiment.lifecycle.git_metadata", return_value=("rev", False)), \
                 mock.patch("tools.opencode_experiment.lifecycle.resolve_cli", return_value=git), \
                 mock.patch("tools.opencode_experiment.lifecycle.subprocess.run", wraps=subprocess.run):
                _root, state, created = prepare("demo", "run", 4567)
            self.assertTrue(created)
            result = subprocess.run(["git", "rev-parse", "HEAD"], cwd=state["workspace"], text=True, stdout=subprocess.PIPE)
            self.assertEqual(result.stdout.strip(), state["plan_revision"])
            self.assertTrue((Path(state["workspace"]) / "experiment.json").is_file())
            self.assertEqual((Path(state["workspace"]) / "bin/tool").read_text(), "tool")
            self.assertEqual(state["opencode_environment"], {})
            self.assertEqual(state["permission_preflight"], {})
            self.assertEqual(state["reporting"], {"sinks": []})
            self.assertEqual(state["metrics"], {"roles": {}})

    def test_reserve_waits_for_start_request_before_preparing(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiments" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            subprocess.run(["git", "init", "--quiet"], cwd=plan, check=True)
            subprocess.run(["git", "add", "."], cwd=plan, check=True)
            subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--quiet", "-m", "plan"], cwd=plan, check=True)
            with mock.patch("tools.opencode_experiment.lifecycle.repository_root", return_value=repo):
                root, state = reserve("demo", "run", 4567)
                self.assertEqual(state["phase"], "waiting")
                self.assertIsNone(state["workspace"])
                self.assertFalse(start_requested(root))
                request_start(root)
                first = (root / "start-request.json").read_text()
                request_start(root)
                self.assertEqual((root / "start-request.json").read_text(), first)
                self.assertTrue(start_requested(root))
                with mock.patch("tools.opencode_experiment.lifecycle.git_metadata", return_value=("repo-rev", False)):
                    _root, prepared, created = prepare("demo", "run", 4567)
                self.assertTrue(created)
                self.assertEqual(prepared["phase"], "preparing")
                self.assertTrue(Path(prepared["workspace"]).is_dir())
                self.assertEqual((Path(prepared["workspace"]) / "bin/tool").read_text(), "tool")

    def test_prepare_rejects_dirty_plan(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiments" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            subprocess.run(["git", "init", "--quiet"], cwd=plan, check=True)
            subprocess.run(["git", "add", "."], cwd=plan, check=True)
            subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--quiet", "-m", "plan"], cwd=plan, check=True)
            (plan / "dirty").write_text("dirty")
            with mock.patch("tools.opencode_experiment.lifecycle.repository_root", return_value=repo), \
                 mock.patch("tools.opencode_experiment.lifecycle.git_metadata", return_value=("rev", False)):
                with self.assertRaisesRegex(ControlError, "clean and committed"):
                    prepare("demo", "run", 4567)


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

    def test_validation_resolves_relative_executable_from_validation_cwd(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            validation_cwd = workspace / "crate"
            validation_cwd.mkdir(parents=True)
            executable = workspace / "bin" / "tool"
            executable.parent.mkdir()
            executable.write_text("#!/bin/sh\necho validated\n")
            executable.chmod(0o755)
            manifest = Manifest(
                "demo", root, {},
                ({"name": "crate", "cwd": "crate", "command": ["../bin/tool"], "required": True},),
                (), (), (), {},
            )
            context = Context(root, root / "execution", {"workspace": str(workspace)}, manifest)

            results = run_validation(context)

            self.assertEqual(results[0]["exit"], 0)
            self.assertEqual(results[0]["stdout"], "validated\n")

    def test_query_prefers_direct_jq_over_mise_jaq(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("tools.opencode_experiment.external.probe_direct", side_effect=lambda cli: (cli,) if cli == "jq" else None), mock.patch("tools.opencode_experiment.external.probe_mise", return_value=None):
            self.assertEqual(select_engine(), ("jq", ["jq"]))


class MetricsTest(unittest.TestCase):
    def test_collects_phases_tokens_waiting_and_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            source = workspace / "output" / "src" / "main.telora"
            source.parent.mkdir(parents=True)
            source.write_text("let x = 1;\nexport { x };\n")
            notes = workspace / "output" / "NOTES.md"
            notes.write_text("# Notes\n\nDone.\n")
            messages = [
                {"info": {"role": "user", "time": {"created": 0}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 1, "completed": 5},
                          "tokens": {"input": 10, "output": 2, "reasoning": 3, "cache": {"read": 7}}}, "parts": []},
                {"info": {"role": "user", "time": {"created": 10}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 11, "completed": 15},
                          "tokens": {"input": 20, "output": 3, "reasoning": 4}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 16, "completed": 20},
                          "tokens": {"input": 30, "output": 5, "reasoning": 6}},
                 "parts": [{"type": "tool", "tool": "write", "state": {"input": {"filePath": str(source)}}}]},
                {"info": {"role": "assistant", "time": {"created": 21, "completed": 25},
                          "tokens": {"input": 40, "output": 7, "reasoning": 8}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 26, "completed": 126},
                          "tokens": {}},
                 "parts": [{"type": "tool", "tool": "bash", "state": {
                     "status": "completed",
                     "input": {"command": "./bin/oc-task pull worker"},
                     "time": {"start": 125, "end": 126},
                 }}]},
            ]
            definition = {"roles": {"worker": {
                "learning_phases": ["language_learning", "api_learning"],
                "work_phase": "implementation",
                "work_files": ["output/src/main.telora"],
                "artifacts": {
                    "code": {"core": ["output/src/*.telora"]},
                    "documents": {"docs": ["output/NOTES.md"]},
                },
            }}}
            children = [{"id": "ses_worker", "agent": "worker", "title": "Worker",
                         "model": {"providerID": "provider", "id": "model", "variant": "v"}}]
            result = collect_metrics("run", "idle", workspace, children, lambda _session: messages, definition)
            role = result["roles"][0]
            self.assertEqual([phase["name"] for phase in role["phases"]],
                             ["language_learning", "api_learning", "implementation"])
            self.assertEqual(role["tokens"]["fresh"], 138)
            self.assertEqual(role["time"], {"first_created": 1, "last_completed": 126,
                                             "active_ms": 16, "span_ms": 125, "waiting_ms": 109})
            self.assertEqual(role["artifacts"]["code"]["total"], {"files": 1, "lines": 2, "bytes": 25})
            self.assertEqual(role["artifacts"]["documents"]["total"]["lines"], 3)
            self.assertEqual(role["productivity"]["code_lines_per_1k_work_fresh_tokens"], 20.833)
            self.assertEqual(result["aggregate"]["phases"]["learning"]["tokens"]["fresh"], 42)
            self.assertEqual(result["aggregate"]["phases"]["work"]["tokens"]["fresh"], 96)
            self.assertEqual(result["aggregate"]["time"]["span_ms"], 125)

    def test_unconfigured_role_is_not_mislabeled_as_learning(self):
        messages = [{"info": {"role": "assistant", "time": {"created": 1, "completed": 2},
                              "tokens": {"input": 3}}, "parts": []}]
        children = [{"id": "ses_worker", "agent": "worker"}]
        result = collect_metrics("run", "idle", Path("/tmp"), children, lambda _session: messages,
                                 {"roles": {}})
        role = result["roles"][0]
        self.assertEqual(role["classification"], {"configured": False, "work_boundary_observed": None})
        self.assertEqual([(phase["name"], phase["kind"]) for phase in role["phases"]],
                         [("unclassified", "unclassified")])
        self.assertEqual(result["aggregate"]["phases"]["unclassified"]["tokens"]["fresh"], 3)

    def test_task_metrics_cover_tokens_thinking_and_telora_commands(self):
        messages = [{
            "info": {"role": "assistant", "time": {"created": 1000, "completed": 2000},
                     "tokens": {"input": 10, "output": 3}},
            "parts": [{"type": "tool", "tool": "bash", "state": {
                "input": {"command": "./bin/telora run main -C demo"},
                "time": {"start": 1300, "end": 1400},
            }}],
        }]
        records = {"active": [], "history": [{
            "task_id": "a1-1", "role": "a1", "artifacts": ["demo.a1"],
            "status": "submitted", "started_at_ns": 900_000_000,
            "submitted_at_ns": 2_100_000_000,
        }]}
        result = collect_metrics(
            "run", "idle", Path("/tmp"), [{"id": "ses_a1", "agent": "a1"}],
            lambda _session: messages, {"roles": {}}, records, now_ms=3000,
        )
        task = result["tasks"][0]
        self.assertEqual(task["tokens"]["fresh"], 13)
        self.assertEqual(task["elapsed_ms"], 1200)
        self.assertEqual(task["longest_thinking_ms"], 600)
        self.assertEqual(task["telora_commands"], 1)

    def test_collects_multiple_work_phases_at_first_matching_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            messages = [
                {"info": {"role": "assistant", "time": {"created": 1, "completed": 2},
                          "tokens": {"input": 1}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 3, "completed": 4},
                          "tokens": {"input": 2}},
                 "parts": [{"type": "tool", "tool": "write", "state": {
                     "input": {"filePath": str(workspace / "model" / "src.telora")}
                 }}]},
                {"info": {"role": "assistant", "time": {"created": 5, "completed": 6},
                          "tokens": {"input": 3}}, "parts": []},
                {"info": {"role": "assistant", "time": {"created": 7, "completed": 8},
                          "tokens": {"input": 4}},
                 "parts": [{"type": "tool", "tool": "edit", "state": {
                     "input": {"filePath": str(workspace / "public" / "query.telora")}
                 }}]},
                {"info": {"role": "assistant", "time": {"created": 9, "completed": 10},
                          "tokens": {"input": 5}}, "parts": []},
            ]
            definition = {"roles": {"worker": {
                "learning_phases": ["learning"],
                "work_phases": [
                    {"name": "modeling", "files": ["model/**"]},
                    {"name": "public_surface", "files": ["public/**"]},
                ],
                "artifacts": {},
            }}}
            children = [{"id": "ses_worker", "agent": "worker"}]
            result = collect_metrics(
                "run", "idle", workspace, children, lambda _session: messages, definition
            )
            phases = result["roles"][0]["phases"]
            self.assertEqual(
                [(phase["name"], phase["tokens"]["fresh"]) for phase in phases],
                [("learning", 1), ("modeling", 5), ("public_surface", 9)],
            )

    def test_stat_reads_live_child_messages(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result_dir = root / "result"
            child_dir = result_dir / "children"
            workspace = result_dir / "workspace"
            child_dir.mkdir(parents=True)
            workspace.mkdir()
            session_id = "ses_worker"
            messages = [
                {"info": {"role": "assistant", "time": {"created": 1, "completed": 2},
                          "tokens": {"input": 7}}, "parts": []},
            ]
            children = [{"id": session_id, "agent": "worker", "title": "Worker"}]
            context = Context(Path(temporary), root, {
                "exec_name": "run", "phase": "idle", "workspace": str(workspace),
                "metrics": {"roles": {}},
            }, mock.Mock(metrics={"roles": {}}))
            output = StringIO()
            with mock.patch("tools.opencode_experiment.cli_ctl.resolve", return_value=context), \
                 mock.patch("tools.opencode_experiment.cli_ctl._live_children",
                            return_value=(children, {session_id: messages}, {})), redirect_stdout(output):
                self.assertEqual(control_main(["stat", "run"]), 0)
            document = json.loads(output.getvalue())
            self.assertEqual(document["execution_phase"], "idle")
            self.assertEqual(document["roles"][0]["tokens"]["fresh"], 7)


class ArchiveExportTest(unittest.TestCase):
    def test_archive_is_repeatable_allows_internal_file_links_and_rejects_escape(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); root = repo / "role"; workspace = repo / "workspace"
            (workspace / "output").mkdir(parents=True); (workspace / "output" / "x").write_text("x")
            manifest = Manifest("demo", repo, {"start": "s", "continue": "c"}, (),
                                ("output",), ("output",), (), {})
            context = Context(repo, root, {"workspace": str(workspace)}, manifest); destination = root / "result" / "workspace"
            copy_archive(context, destination); self.assertTrue((destination / "output/x").is_file())
            copy_archive(context, destination); self.assertTrue((destination / "output/x").is_file())
            os.symlink("x", workspace / "output" / "internal")
            copy_archive(context, destination)
            self.assertEqual((destination / "output/internal").read_text(), "x")
            os.symlink("/tmp", workspace / "output" / "escape")
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


class PermissionPreflightTest(unittest.TestCase):
    def manifest(self, root: Path, commands: tuple[str, ...]) -> Manifest:
        return Manifest("demo", root, {"start": "s", "continue": "c"}, (), (), (), (), {},
                        {"worker": commands})

    def workspace(self, root: Path, permission: object) -> Path:
        workspace = root / "ws"; agents = workspace / ".opencode" / "agents"
        agents.mkdir(parents=True)
        (workspace / "opencode.json").write_text(json.dumps({"permission": "deny"}))
        (agents / "worker.md").write_text(
            f"---\npermission: {json.dumps(permission, separators=(',', ':'))}\n---\nWorker.\n"
        )
        return workspace

    def test_accepts_declared_best_effort_command(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = self.workspace(root, {"bash": {
                "*": "deny", "./bin/telora run * -C ontology *": "allow",
            }})
            command = "./bin/telora run invalid -C ontology --best-effort"
            result = preflight_permissions(self.manifest(root, (command,)), workspace)
            self.assertEqual(result["worker"], [{"command": command, "decision": "allow"}])

    def test_rejects_deny_and_ask(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = self.workspace(root, {"bash": {"*": "deny"}})
            with self.assertRaisesRegex(ControlError, "rejected worker command"):
                preflight_permissions(self.manifest(root, ("./bin/telora run main",)), workspace)
            workspace = self.workspace(root / "ask", {"bash": {"*": "ask"}})
            with self.assertRaisesRegex(ControlError, "interactive permission"):
                preflight_permissions(self.manifest(root, ()), workspace)


class WatchTest(unittest.TestCase):
    def test_window_debounce_timeout_and_finish(self):
        empty = WatchWindow(10, 30, 300)
        self.assertIsNone(empty.reason(309))
        self.assertEqual(empty.reason(310), "timeout")
        active = WatchWindow(10, 30, 300); active.add("one", {"kind": "file_start"}, 20)
        self.assertIsNone(active.reason(49))
        self.assertEqual(active.reason(50), "debounced")
        self.assertEqual(active.reason(21, finished=True), "experiment_finished")

    def test_reasoning_is_ignored_and_tool_states_are_distinct(self):
        messages = [{"info": {"id": "msg", "role": "assistant"}, "parts": [
            {"type": "reasoning", "text": "secret"},
            {"id": "tool", "type": "tool", "tool": "bash",
             "state": {"status": "running", "input": {"command": "echo ok"}}},
        ]}]
        started = message_events("ses", "a2", messages)
        self.assertEqual([event[1]["kind"] for event in started], ["command_start"])
        messages[0]["parts"][1]["state"] = {
            "status": "completed", "input": {"command": "echo ok"}, "metadata": {"exit": 0},
        }
        completed = message_events("ses", "a2", messages)
        self.assertEqual([event[1]["kind"] for event in completed], ["command_result"])
        self.assertNotEqual(started[0][0], completed[0][0])

    def test_permission_event_is_infrastructure_error(self):
        events = acp_events({"type": "permission.asked", "properties": {"sessionID": "ses"}}, {})
        self.assertEqual(events[0][1]["kind"], "infrastructure_permission_error")

    def test_persisted_cursor_deduplicates_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); (root / "plan").write_text("demo\n")
            save_state(root, {"schema": SCHEMA, "plan_id": "demo", "exec_name": "run",
                               "phase": "finished", "workspace": "/tmp/ws",
                               "server_url": "http://127.0.0.1:1", "session_id": "ses"})
            manifest = Manifest("demo", root, {"start": "s", "continue": "c"}, (), (), (), (), {})
            context = Context(root, root, load_state(root), manifest)
            client = mock.Mock()
            client.messages.return_value = [{"info": {"id": "msg", "role": "assistant"}, "parts": [
                {"id": "tool", "type": "tool", "tool": "read",
                 "state": {"status": "completed", "input": {"filePath": "GOAL.md"}}},
            ]}]
            client.children.return_value = []
            with mock.patch.object(Context, "client", return_value=client):
                first = watch_progress(context, 30, 300)
                second = watch_progress(context, 30, 300)
            self.assertEqual(len(first["events"]), 1)
            self.assertEqual(second["events"], [])
            self.assertEqual(first["next_cursor"], "1")
            self.assertEqual(second["next_cursor"], "2")


class ReportingTest(unittest.TestCase):
    def context(self, root: Path, sinks: list[dict]) -> Context:
        manifest = Manifest("demo", root, {"start": "s", "continue": "c"}, (), (), (), (), {})
        state = {"exec_name": "run", "reporting": {"sinks": sinks}}
        return Context(root, root / "execution", state, manifest)

    def test_without_sink_only_persists_locally(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); body = root / "body.md"; body.write_text("Working.")
            with mock.patch("tools.opencode_experiment.reporting.resolve_cli") as resolve:
                record = submit_report(self.context(root, []), body)
            resolve.assert_not_called()
            self.assertEqual(record["status"], "ok")
            stored = root / "execution" / "reports" / "000.md"
            self.assertIn("未经 Host 验收", stored.read_text())

    def test_github_sink_uses_body_file_and_records_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); body = root / "body.md"; body.write_text("Working.")
            sink = {"kind": "github_issue_comment", "repository": "owner/repo", "issue": 7}
            failed = subprocess.CompletedProcess([], 1, "", "offline")
            with mock.patch("tools.opencode_experiment.reporting.resolve_cli", return_value=("gh",)), \
                 mock.patch("tools.opencode_experiment.reporting.subprocess.run", return_value=failed) as run:
                record = submit_report(self.context(root, [sink]), body)
            self.assertEqual(record["status"], "error")
            command = run.call_args.args[0]
            self.assertIn("--body-file", command)
            self.assertNotIn("--body", command)
            self.assertEqual(record["sinks"][0]["error"], "offline")


if __name__ == "__main__": unittest.main()
