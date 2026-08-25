from __future__ import annotations

import json
import os
import socket
import subprocess
import tempfile
import threading
import unittest
from contextlib import redirect_stderr, redirect_stdout
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import StringIO
from pathlib import Path
from unittest import mock

from tools.opencode_experiment.client import Client
from tools.opencode_experiment.config import ControlError, Manifest, load_manifest, safe_relative, validate_identifier
from tools.opencode_experiment.observe import failures, latest_assistant, normalized, summarize
from tools.opencode_experiment.query import select_engine
from tools.opencode_experiment.external import probe_direct, probe_mise, resolve_capabilities, resolve_cli, resolve_command
from tools.opencode_experiment.state import (
    SCHEMA,
    atomic_json,
    bind_plan,
    create_runner_config,
    create_run_config,
    load_connect_test,
    load_run_config,
    load_runner_config,
    load_state,
    record_connect_test,
    save_state,
)
from tools.opencode_experiment.lifecycle import (
    _inherit_execution,
    _inheritance_compatible,
    copy_archive,
    export_session,
    opencode_environment,
    prepare,
    probe_opencode_connection,
    request_start,
    reserve,
    run_validation,
    start_requested,
)
from tools.opencode_experiment.runtime_opencode import ENVIRONMENT, MODEL, generate
from tools.opencode_experiment.metrics import collect_metrics
from tools.opencode_experiment.context import Context
from tools.opencode_experiment.events import event_detail, project_events
from tools.opencode_experiment.permissions import preflight_permissions
from tools.opencode_experiment.reporting import submit_report
from tools.opencode_experiment.task_cli import evaluate, publish_artifact, pull, submit, validate_workflow, workflow_status
from tools.opencode_experiment.watch import WatchWindow, acp_events, message_events, watch_progress
from tools.opencode_experiment.cli_ctl import (
    _configure_start,
    _host_pull,
    _publish,
    _resume,
    _role_output_owners,
    _status,
    _test_connect,
    _update,
    main as control_main,
    parser as control_parser,
)
from tools.opencode_experiment.cli_run import main as run_main, parser as run_parser


class Handler(BaseHTTPRequestHandler):
    messages: list[dict] = []
    last_payload: dict = {}

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
        self.__class__.last_payload = payload
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
        Client(self.client.url, "/tmp/ws").create_session("child", "ses_parent")
        self.assertEqual(Handler.last_payload, {"title": "child", "parentID": "ses_parent"})
        self.client.prompt("hello"); self.assertEqual(self.client.messages()[-1]["parts"][0]["text"], "hello")
        self.client.prompt_session("ses_test", "continue"); self.assertEqual(self.client.messages()[-1]["parts"][0]["text"], "continue")

    def test_loopback_only(self):
        with self.assertRaises(ControlError): Client("http://example.com:12", "/tmp/ws")

    def test_unavailable(self):
        with self.assertRaises(ControlError): Client("http://127.0.0.1:1", "/tmp/ws", timeout=.01).health()


class ConfigStateTest(unittest.TestCase):
    PULL_MESSAGE = [{
        "info": {"role": "assistant"},
        "parts": [{"type": "tool", "tool": "bash", "state": {
            "status": "running", "input": {"command": "./bin/oc-task pull a5"},
        }}],
    }]

    @staticmethod
    def write_plan(plan: Path) -> None:
        plan.mkdir(parents=True)
        (plan / "roles").mkdir()
        (plan / "host").mkdir()
        (plan / "roles" / "a1.md").write_text("Keep pulling work.\n", encoding="utf-8")
        (plan / "host" / "secret.md").write_text("hidden\n", encoding="utf-8")
        (plan / "seed.txt").write_text("seed\n", encoding="utf-8")
        (plan / "experiment.json").write_text(json.dumps({
            "schema": "telora.experiment-plan/v1",
            "workspace": ["seed.txt"],
            "roles": {"a1": {
                "description": "worker", "instructions": "roles/a1.md",
                "read": ["seed.txt"], "write": ["output.txt"],
                "commands": ["./bin/oc-task pull a1", "./bin/oc-task submit a1 *"],
                "preflight": ["./bin/oc-task pull a1", "./bin/oc-task submit a1 *"],
            }},
            "artifacts": [{"name": "tool", "source": "tool", "to": "bin/tool", "mode": "0555"}],
            "validation": [], "observe": ["bin"], "archive": ["bin", "opencode.json", "experiment.json"],
        }))

    @staticmethod
    def commit_repo(repo: Path) -> None:
        subprocess.run(["git", "init", "--quiet"], cwd=repo, check=True)
        subprocess.run(["git", "add", "."], cwd=repo, check=True)
        subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com",
                        "commit", "--quiet", "-m", "plan"], cwd=repo, check=True)

    def test_identifiers_and_paths(self):
        self.assertEqual(validate_identifier("a2-001", "exec"), "a2-001")
        for value in ("../x", "/x", ".", "a/../b"):
            with self.assertRaises(ControlError): safe_relative(value)
        for value in ("A", "a/b", ".hidden", "a b"):
            with self.assertRaises(ControlError): validate_identifier(value, "id")

    def test_manifest_rejects_unknown_telora_preflight_subcommand(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            path = plan / "experiment.json"
            manifest = json.loads(path.read_text(encoding="utf-8"))
            manifest["roles"]["a1"]["preflight"] = ["./bin/telora types --limit 20"]
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(
                ControlError,
                r"unsupported Telora subcommand.*'types'.*check, lsp, query, run",
            ):
                load_manifest(repo, "demo")

            manifest["roles"]["a1"]["preflight"] = [
                "./bin/telora query exports @bin/main -C query-builder",
            ]
            path.write_text(json.dumps(manifest), encoding="utf-8")
            self.assertEqual(
                load_manifest(repo, "demo").permission_preflight["a1"],
                ("./bin/telora query exports @bin/main -C query-builder",),
            )

    def test_artifact_publication_command_is_available(self):
        args = control_parser().parse_args(["publish", "run", "draft", "result"])
        self.assertEqual((args.command, args.test_id, args.artifacts),
                         ("publish", "run", ["draft", "result"]))
        forced = control_parser().parse_args(["publish", "run", "draft", "--force"])
        self.assertTrue(forced.force)
        forced_update = control_parser().parse_args(
            ["update", "run", "output.txt=input.txt", "--force"]
        )
        self.assertTrue(forced_update.force)
        forced_resume = control_parser().parse_args(["resume", "run", "a1", "--force"])
        self.assertTrue(forced_resume.force)

    def test_stat_command_is_available(self):
        args = control_parser().parse_args(["stat", "run"])
        self.assertEqual((args.command, args.test_id), ("stat", "run"))

    def test_control_surface_includes_connection_preflight(self):
        self.assertEqual(set(control_parser()._subparsers._group_actions[0].choices),
                         {"test-connect", "start", "stat", "status", "pull", "event", "update", "publish", "resume"})
        args = control_parser().parse_args(["pull", "run", "123", "--timeout", "5"])
        self.assertEqual((args.test_id, args.since, args.timeout), ("run", 123, 5.0))
        event = control_parser().parse_args(["event", "run", "task:a1-1"])
        self.assertEqual((event.test_id, event.event_id), ("run", "task:a1-1"))

    def test_resume_targets_an_existing_inactive_role(self):
        client = mock.Mock()
        client.children.return_value = [{"id": "ses_a5", "agent": "a5"}]
        client.statuses.side_effect = [
            {"ses_a5": {"type": "idle"}},
            {"ses_a5": {"type": "busy"}},
        ]
        client.create_session.return_value = {"id": "ses_a5_new"}
        client.session_messages.return_value = self.PULL_MESSAGE
        context = mock.Mock()
        context.state = {"exec_name": "run-001", "workflow": {"roles": ["a5"]},
                         "session_id": "ses_coordinator"}
        context.client.return_value = client

        result = _resume(context, "a5", .01)
        self.assertEqual(result["session_id"], "ses_a5")
        self.assertTrue(result["loop_observed"])
        client.prompt_session.assert_called_once()

    def test_resume_is_idempotent_for_a_busy_role(self):
        client = mock.Mock()
        client.children.return_value = [{"id": "ses_a5", "agent": "a5"}]
        client.statuses.return_value = {"ses_a5": {"type": "busy"}}
        client.session_messages.return_value = self.PULL_MESSAGE
        context = mock.Mock()
        context.state = {"exec_name": "run-001", "workflow": {"roles": ["a5"]}}
        context.client.return_value = client

        self.assertEqual(_resume(context, "a5")["action"], "already_running")
        client.prompt_session.assert_not_called()

    def test_force_resume_aborts_and_restarts_a_busy_role(self):
        client = mock.Mock()
        old = {"id": "ses_a5_old", "agent": "a5"}
        new = {"id": "ses_a5_new", "agent": "a5"}
        client.children.side_effect = [[old], [old, new]]
        client.statuses.side_effect = [
            {"ses_a5_old": {"type": "busy"}},
            {"ses_a5_old": {"type": "idle"}},
            {"ses_a5_old": {"type": "idle"}, "ses_a5_new": {"type": "busy"}},
        ]
        client.session_messages.return_value = self.PULL_MESSAGE
        client.create_session.return_value = {"id": "ses_a5_new"}
        context = mock.Mock()
        context.state = {"exec_name": "run-001", "workflow": {"roles": ["a5"]},
                         "session_id": "ses_coordinator"}
        context.client.return_value = client

        result = _resume(context, "a5", .01, force=True)

        self.assertEqual((result["action"], result["session_id"]),
                         ("recreated", "ses_a5_new"))
        client.abort_session.assert_called_once_with("ses_a5_old")
        client.prompt_session.assert_called_once_with(
            "ses_a5_new", mock.ANY, agent="a5"
        )
        client.create_session.assert_called_once_with(
            "恢复 A5 角色循环", parent_id="ses_coordinator"
        )

    def test_resume_rejects_unknown_role_and_recreates_missing_session(self):
        context = mock.Mock()
        context.state = {"exec_name": "run-001", "workflow": {"roles": ["a5"]}}

        with self.assertRaisesRegex(ControlError, "unknown workflow role"):
            _resume(context, "a4")

        context.state["session_id"] = "ses_coordinator"
        client = mock.Mock()
        client.children.side_effect = [[], [{"id": "ses_a5_new", "agent": "a5"}]]
        client.statuses.side_effect = [{}, {"ses_a5_new": {"type": "busy"}}]
        client.session_messages.return_value = self.PULL_MESSAGE
        client.create_session.return_value = {"id": "ses_a5_new"}
        context.client.return_value = client
        result = _resume(context, "a5", .01)
        self.assertEqual((result["action"], result["session_id"]), ("recreated", "ses_a5_new"))
        client.prompt_session.assert_called_once_with(
            "ses_a5_new", mock.ANY, agent="a5"
        )

    def test_resume_replaces_an_existing_session_that_does_not_reenter_loop(self):
        client = mock.Mock()
        old = {"id": "ses_a5_old", "agent": "a5"}
        new = {"id": "ses_a5_new", "agent": "a5"}
        client.children.side_effect = [[old], [old], [old, new]]
        client.statuses.side_effect = [
            {"ses_a5_old": {"type": "idle"}},
            {"ses_a5_old": {"type": "idle"}},
            {"ses_a5_old": {"type": "idle"}, "ses_a5_new": {"type": "busy"}},
        ]
        client.session_messages.return_value = self.PULL_MESSAGE
        client.create_session.return_value = {"id": "ses_a5_new"}
        context = mock.Mock()
        context.state = {"exec_name": "run-001", "workflow": {"roles": ["a5"]},
                         "session_id": "ses_coordinator"}
        context.client.return_value = client
        with mock.patch("tools.opencode_experiment.cli_ctl.time.monotonic",
                        side_effect=[0, 1, 1, 1]):
            result = _resume(context, "a5", .5)
        self.assertEqual((result["action"], result["session_id"]), ("recreated", "ses_a5_new"))
        self.assertEqual(client.prompt_session.call_args_list[0].args[0], "ses_a5_old")
        self.assertEqual(client.prompt_session.call_args_list[1].args[0], "ses_a5_new")

    def test_start_requires_test_and_plan_identity(self):
        args = control_parser().parse_args(["start", "ontology-3-009", "ontology-3"])
        self.assertEqual(
            (args.command, args.test_id, args.plan_id, args.from_test_id),
            ("start", "ontology-3-009", "ontology-3", None),
        )
        inherited = control_parser().parse_args(
            ["start", "ontology-3-010", "ontology-3", "--from", "ontology-3-009"]
        )
        self.assertEqual(inherited.from_test_id, "ontology-3-009")

    def test_run_requires_test_id_and_reserved_port(self):
        args = run_parser().parse_args(["ontology-3-006", "4199"])
        self.assertEqual(vars(args), {"test_id": "ontology-3-006", "port": 4199})

    def test_run_reports_an_occupied_port_before_waiting_for_host(self):
        with socket.socket() as occupied:
            occupied.bind(("127.0.0.1", 0))
            occupied.listen(1)
            port = occupied.getsockname()[1]
            stderr = StringIO()
            with mock.patch(
                "tools.opencode_experiment.cli_run.resolve_cli", return_value=("opencode",)
            ), redirect_stderr(stderr):
                result = run_main(["run-001", str(port)])
        self.assertEqual(result, 69)
        self.assertIn(f"cannot reserve runner port {port}", stderr.getvalue())

    def test_host_configures_the_explicit_plan_and_runner_port(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            record_connect_test(repo, "run-001", {"health": True, "session_id": "ses_probe"})
            create_runner_config(repo, "run-001", 43123)
            value = _configure_start(repo, "run-001", "demo")
            self.assertEqual(value["plan_id"], "demo")
            self.assertEqual(value["port"], 43123)
            self.assertEqual(load_run_config(repo, "run-001"), value)
            self.assertEqual(create_run_config(repo, "run-001", "demo", 49999), value)

    def test_run_configuration_records_inherited_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            value = create_run_config(repo, "run-002", "demo", 4199, "run-001")
            self.assertEqual(value["from_test_id"], "run-001")
            self.assertEqual(load_run_config(repo, "run-002"), value)
            with self.assertRaisesRegex(ControlError, "another source"):
                create_run_config(repo, "run-002", "demo", 4199, "other")

    def test_inheritance_copies_only_current_unchanged_artifact_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiment-plans" / "demo"
            plan.mkdir(parents=True)
            workflow = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1", "a5"],
                "start_artifacts": ["lang"],
                "finish_artifact": "answer",
                "artifacts": {
                    "lang": {"desc": "language", "checks": ["GOAL.md"]},
                    "feedback": {"desc": "feedback", "checks": ["FEEDBACK.md"]},
                    "draft.a1": {"desc": "draft", "input": ["lang", "feedback?"],
                                 "checks": ["output.txt"], "instruction": "build"},
                    "approved": {"desc": "approved", "input": ["draft.a1"]},
                    "homework.a5": {"desc": "homework", "input": ["approved"],
                                    "instruction": "answer"},
                    "answer": {"desc": "answer", "input": ["homework.a5"]},
                },
            })
            source_root = bind_plan(repo, "demo", "old")
            source_workspace = repo / "old-workspace"
            source_workspace.mkdir()
            (source_workspace / "GOAL.md").write_text("old language", encoding="utf-8")
            publish_artifact(source_workspace, workflow, "lang")
            (source_workspace / "FEEDBACK.md").write_text("accepted feedback", encoding="utf-8")
            publish_artifact(source_workspace, workflow, "feedback")
            (source_workspace / "output.txt").write_text("accepted output", encoding="utf-8")
            pull(source_workspace, workflow, "a1", False, None)
            submit(source_workspace, workflow, "a1", ["draft.a1"])
            publish_artifact(source_workspace, workflow, "approved")
            save_state(source_root, {
                "schema": SCHEMA, "plan_id": "demo", "exec_name": "old", "phase": "idle",
                "workspace": str(source_workspace), "workflow": workflow,
            })

            target = repo / "new-workspace"
            target.mkdir()
            (target / "GOAL.md").write_text("old language", encoding="utf-8")
            result = _inherit_execution(repo, "old", "demo", target, workflow)
            self.assertEqual(result["artifacts"], ["lang", "feedback", "draft.a1", "approved"])
            self.assertEqual((target / "output.txt").read_text(), "accepted output")
            self.assertEqual((target / "FEEDBACK.md").read_text(), "accepted feedback")
            self.assertEqual((target / "GOAL.md").read_text(), "old language")
            status = evaluate(target, workflow)["artifacts"]
            self.assertTrue(status["approved"]["current"])
            self.assertTrue(status["homework.a5"]["runnable"])
            self.assertFalse((target / ".oc-task" / "active").exists())
            self.assertFalse((target / ".oc-task" / "history").exists())

    def test_inheritance_accepts_only_preexisting_host_gate_strengthening(self):
        old = {
            "id": "approved", "desc": "approved", "owner": None,
            "input": [{"id": "draft.a1", "optional": False}],
            "checks": [], "instruction": None,
        }
        new = {**old, "input": [
            {"id": "draft.a1", "optional": False},
            {"id": "review.a2", "optional": False},
        ]}
        approval = {"stamp_mtime_ns": 30}
        statuses = {"review.a2": {"current": True, "stamp_mtime_ns": 20}}
        self.assertTrue(_inheritance_compatible(old, new, approval, statuses))
        statuses["review.a2"]["stamp_mtime_ns"] = 40
        self.assertFalse(_inheritance_compatible(old, new, approval, statuses))
        new["desc"] = "changed semantics"
        self.assertFalse(_inheritance_compatible(old, new, approval, statuses))

    def test_runner_configuration_records_the_external_port(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            value = create_runner_config(repo, "run-001", 4199)
            self.assertEqual(load_runner_config(repo, "run-001"), value)
            self.assertEqual(value["port"], 4199)
            with self.assertRaisesRegex(ControlError, "another port"):
                create_runner_config(repo, "run-001", 4200)

    def test_start_requires_a_connection_test_before_freezing_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            create_runner_config(repo, "run-001", 43123)
            with self.assertRaisesRegex(ControlError, "test-connect"):
                _configure_start(repo, "run-001", "demo")
            self.assertFalse((repo / "target/exp/run-001/config.json").exists())

    def test_start_requires_the_external_runner_after_connection_preflight(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            record_connect_test(repo, "run-001", {"health": True, "session_id": "ses_probe"})
            with self.assertRaisesRegex(ControlError, "oc-run run-001 <port>"):
                _configure_start(repo, "run-001", "demo")
            self.assertFalse((repo / "target/exp/run-001/config.json").exists())

    def test_start_rejects_a_config_that_differs_from_the_reserved_port(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            record_connect_test(repo, "run-001", {"health": True, "session_id": "ses_probe"})
            create_runner_config(repo, "run-001", 4199)
            create_run_config(repo, "run-001", "demo", 4200)
            plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            with self.assertRaisesRegex(ControlError, "does not match"):
                _configure_start(repo, "run-001", "demo")

    def test_connect_records_a_receipt_without_releasing_oc_run(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            create_runner_config(repo, "run-001", 4199)
            with mock.patch(
                "tools.opencode_experiment.cli_ctl.probe_opencode_connection",
                return_value={"health": True, "session_id": "ses_probe"},
            ) as probe:
                receipt = _test_connect(repo, "run-001")
            self.assertEqual(load_connect_test(repo, "run-001"), receipt)
            self.assertFalse((repo / "target/exp/run-001/config.json").exists())
            probe.assert_called_once_with(
                "run-001", 4199, repo / "target/exp/run-001/runner-workspace"
            )

    def test_connect_rejects_an_already_configured_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            create_run_config(repo, "run-001", "demo", 43123)
            with self.assertRaisesRegex(ControlError, "already configured"):
                _test_connect(repo, "run-001")

    def test_connection_probe_exercises_the_runner_health_and_session(self):
        client = mock.Mock()
        client.health.return_value = {"healthy": True}
        client.create_session.return_value = {"id": "ses_probe"}
        workspace = Path("/tmp/runner-workspace")
        with mock.patch("tools.opencode_experiment.lifecycle.Client", return_value=client) as factory:
            result = probe_opencode_connection("run-001", 4199, workspace)
        self.assertEqual(result, {"health": True, "session_id": "ses_probe"})
        factory.assert_called_once_with("http://127.0.0.1:4199", str(workspace), timeout=0.5)
        client.health.assert_called_once()
        client.create_session.assert_called_once()

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

    def test_role_output_check_matches_direct_and_nested_glob_paths(self):
        workflow = {
            "artifacts": {
                "ontology.a3": {
                    "owner": "a3",
                    "checks": ["ontology/src/**/*.telora"],
                },
            },
        }
        self.assertEqual(_role_output_owners(workflow, "ontology/src/ontology.telora"), ["a3"])
        self.assertEqual(_role_output_owners(workflow, "ontology/src/bin/main.telora"), ["a3"])

    def test_force_update_and_publish_cross_role_ownership_and_record_events(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = root / "workspace"
            workspace.mkdir()
            source = root / "replacement.txt"
            source.write_text("replacement", encoding="utf-8")
            workflow = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1"],
                "start_artifacts": ["lang"],
                "finish_artifact": "accepted",
                "artifacts": {
                    "lang": {"desc": "input"},
                    "draft.a1": {"desc": "draft", "input": ["lang"],
                                 "checks": ["output.txt"], "instruction": "build"},
                    "accepted": {"desc": "accepted", "input": ["draft.a1"]},
                },
            })
            context = mock.Mock()
            context.root = root / "execution"
            context.state = {"exec_name": "run", "phase": "idle",
                             "workspace": str(workspace), "workflow": workflow}
            with self.assertRaisesRegex(ControlError, "requires --force"):
                _update(context, [f"output.txt={source}"])
            updated = _update(context, [f"output.txt={source}"], force=True)
            self.assertTrue(updated[0]["host_forced"])
            removed = _publish(context, ["draft.a1=!"], force=True)
            self.assertTrue(removed[0]["host_forced"])
            events = list((context.root / "host-interventions").glob("*.json"))
            self.assertEqual(len(events), 2)
            self.assertEqual(
                len(list((workspace / "control/host-interventions").glob("*.json"))), 2
            )

    def test_atomic_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); (root / "plan").write_text("plan\n")
            state = {"schema": SCHEMA, "plan_id": "plan", "exec_name": "run", "phase": "ready"}; save_state(root, state)
            self.assertEqual(load_state(root), state)

    def test_ontology_3_pins_model_and_uses_file_driven_workflow(self):
        repo = Path(__file__).resolve().parents[3]
        plan = repo / "experiment-plans" / "ontology-3"
        self.assertFalse((plan / "opencode.json").exists())
        self.assertFalse((plan / ".opencode").exists())
        manifest = load_manifest(repo, "ontology-3")
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            first = generate(manifest, workspace)
            first_content = {path: (workspace / path).read_bytes() for path in first}
            second = generate(manifest, workspace)
            self.assertEqual(first, second)
            self.assertEqual(first_content, {path: (workspace / path).read_bytes() for path in second})
            self.assertEqual(json.loads((workspace / "opencode.json").read_text())["model"], MODEL)
            coordinator = (workspace / ".opencode/agents/coordinator.md").read_text()
            self.assertIn("同时启动 A1、A2、A3、A4、A5 各一次", coordinator)
            a4 = (workspace / ".opencode/agents/a4.md").read_text(encoding="utf-8")
            a4_permission_line = next(
                line.removeprefix("permission: ")
                for line in a4.splitlines()
                if line.startswith("permission: ")
            )
            a4_permissions = json.loads(a4_permission_line)
            self.assertEqual(a4_permissions["read"]["*"], "deny")
            self.assertNotIn("ent-1/**", a4_permissions["read"])
            self.assertEqual(a4_permissions["edit"]["*"], "deny")
            self.assertEqual(a4_permissions["edit"]["intent-1/intent.json"], "allow")
            self.assertEqual(a4_permissions["edit"]["**/intent-1/invalid/**"], "allow")
            self.assertNotIn("intent-1/src/**", a4_permissions["edit"])
            self.assertEqual(a4_permissions["read"]["**/experiment.json"], "deny")
            self.assertNotIn("docs/**", a4_permissions["read"])
            self.assertEqual(
                a4_permissions["bash"]["just a4 *"],
                "allow",
            )
            self.assertFalse(any(command.startswith("./bin/telora")
                                 for command in a4_permissions["bash"]))
            a5 = (workspace / ".opencode/agents/a5.md").read_text(encoding="utf-8")
            a5_permission_line = next(
                line.removeprefix("permission: ")
                for line in a5.splitlines()
                if line.startswith("permission: ")
            )
            a5_permissions = json.loads(a5_permission_line)
            self.assertEqual(a5_permissions["edit"]["query-1/answers/*.json"], "allow")
            self.assertEqual(a5_permissions["bash"]["just a5 *"], "allow")
            self.assertFalse(any(command.startswith("./bin/telora")
                                 for command in a5_permissions["bash"]))
        self.assertEqual(
            [phase["name"] for phase in manifest.metrics["roles"]["a3"]["work_phases"]],
            ["modeling", "query_surface_design"],
        )
        workflow = manifest.workflow
        self.assertEqual(workflow["schema"], "telora.artifact-workflow/v1")
        self.assertEqual(workflow["start_artifacts"],
                         ["lang", "qb-req", "edsl-req", "domain-ent-1", "intent-req", "homework"])
        self.assertEqual(workflow["finish_artifact"], "answer")
        artifacts = workflow["artifacts"]
        self.assertEqual(artifacts["qb.a1"]["owner"], "a1")
        self.assertEqual(artifacts["edsl.a2"]["owner"], "a2")
        self.assertEqual(artifacts["ent-1-model.a3"]["owner"], "a3")
        self.assertEqual(artifacts["intent-1.a4"]["owner"], "a4")
        self.assertEqual(artifacts["homework.a5"]["owner"], "a5")
        self.assertEqual(artifacts["answer.a5"]["owner"], "a5")
        self.assertIsNone(artifacts["lic"]["owner"])
        self.assertIsNone(artifacts["qb"]["owner"])
        self.assertEqual(artifacts["qb.a1"]["input"], [
            {"id": "lang", "optional": False},
            {"id": "qb-req", "optional": False},
            {"id": "qb-feedback", "optional": True},
        ])
        self.assertEqual(artifacts["qb-feedback.a2"]["owner"], "a2")
        self.assertEqual(artifacts["qb-feedback.a3"]["owner"], "a3")
        self.assertIsNone(artifacts["qb-feedback"]["owner"])
        self.assertEqual(artifacts["qb"]["input"], [
            {"id": "qb.a1", "optional": False},
            {"id": "qb-feedback.a2", "optional": False},
            {"id": "qb-feedback.a3", "optional": False},
        ])
        self.assertEqual(artifacts["edsl"]["input"], [
            {"id": "edsl.a2", "optional": False},
            {"id": "edsl-feedback.a3", "optional": False},
        ])
        self.assertEqual(artifacts["ent-1-query-surface"]["input"], [
            {"id": "ent-1-query-surface.a3", "optional": False},
            {"id": "ent-1-query-surface-feedback.a4", "optional": False},
        ])
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
        self.assertIn("just a4 expect-invalid *", manifest.permission_preflight["a4"])
        self.assertIn("just a5 make-query *", manifest.permission_preflight["a5"])
        for role in ("a1", "a2", "a3", "a4"):
            text = (plan / "roles" / f"{role}.md").read_text(encoding="utf-8")
            self.assertNotIn("stopped: true", text)
            self.assertNotIn("telora types", text)
        self.assertNotIn("stop_path", workflow)
        self.assertFalse(any("mark-blocked" in command for commands in manifest.permission_preflight.values()
                             for command in commands))
        ontology_goal = (plan / "ontology" / "GOAL.md").read_text(encoding="utf-8")
        intent_goal = (plan / "intent-1" / "GOAL.md").read_text(encoding="utf-8")
        self.assertIn("Top N", ontology_goal)
        self.assertIn("bindings", ontology_goal)
        self.assertIn("bindings", intent_goal)

        query_task = plan / "query-1" / "query_task.py"
        listed = subprocess.run(
            ["python3", str(query_task), "a5", "", ""],
            cwd=plan,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(listed.returncode, 0)
        self.assertIn("make-query <problem-id>", listed.stdout)
        rejected = subprocess.run(
            ["python3", str(query_task), "a5", "make-query", "../0001"],
            cwd=plan,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(rejected.returncode, 64)
        self.assertTrue((plan / "host/A5-HARD-QUERIES.md").is_file())
        self.assertEqual(len(list((plan / "host/a5-cases").glob("*.problem.md"))), 10)
        self.assertNotIn("host", manifest.workspace)

    def test_opencode_environment_is_adapter_owned(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            manifest = load_manifest(repo, "demo")
            self.assertNotIn("environment", json.loads((plan / "experiment.json").read_text()))
            self.assertEqual(opencode_environment({})["OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX"],
                             ENVIRONMENT["OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX"])

    def test_manifest_validates_metrics(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiment-plans" / "demo"
            self.write_plan(plan)
            path = plan / "experiment.json"
            data = json.loads(path.read_text())
            data["metrics"] = {"roles": {"a1": {
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
            self.assertEqual(metrics["roles"]["a1"]["work_phase"], "implementation")
            self.assertEqual(metrics["roles"]["a1"]["artifacts"]["code"]["core"], ["output/src/*.telora"])

    def test_prepare_copies_tracked_plan_and_generates_runtime_adapter(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiment-plans" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            self.commit_repo(repo)
            git = ("git",)
            with mock.patch("tools.opencode_experiment.lifecycle.repository_root", return_value=repo), \
                 mock.patch("tools.opencode_experiment.lifecycle.git_metadata", return_value=("rev", False)), \
                 mock.patch("tools.opencode_experiment.lifecycle.resolve_cli", return_value=git), \
                 mock.patch("tools.opencode_experiment.lifecycle.subprocess.run", wraps=subprocess.run):
                _root, state, created = prepare("demo", "run", 4567)
            self.assertTrue(created)
            workspace = Path(state["workspace"])
            self.assertTrue((workspace / "experiment.json").is_file())
            self.assertTrue((workspace / ".opencode/agents/a1.md").is_file())
            self.assertEqual((workspace / "bin/tool").read_text(), "tool")
            self.assertEqual((workspace / "seed.txt").read_text(), "seed\n")
            self.assertFalse((workspace / "host/secret.md").exists())
            self.assertEqual(state["opencode_environment"], ENVIRONMENT)
            self.assertEqual(set(state["permission_preflight"]), {"a1"})
            self.assertEqual(state["reporting"], {"sinks": []})
            self.assertEqual(state["metrics"], {"roles": {}})

    def test_reserve_waits_for_start_request_before_preparing(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); plan = repo / "experiment-plans" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            self.commit_repo(repo)
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
            repo = Path(temporary); plan = repo / "experiment-plans" / "demo"; self.write_plan(plan)
            artifact = repo / "tool"; artifact.write_text("tool")
            self.commit_repo(repo)
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
                "demo", root, (), {},
                ({"name": "crate", "cwd": "crate", "command": ["../bin/tool"], "required": True},),
                (), (), (),
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

    def test_metrics_warn_when_configured_files_and_work_boundary_are_missing(self):
        messages = [{
            "info": {"role": "assistant", "time": {"created": 1000, "completed": 2000},
                     "tokens": {"input": 1}},
            "parts": [],
        }]
        definition = {"roles": {"worker": {
            "learning_phases": ["learning"],
            "work_phase": "implementation",
            "work_files": ["missing/src/*.telora"],
            "artifacts": {"code": {"core": ["missing/src/*.telora"]}},
        }}}
        records = {"active": [], "history": [{
            "task_id": "worker-1", "role": "worker", "artifacts": ["build.worker"],
            "status": "submitted", "started_at_ns": 900_000_000,
            "submitted_at_ns": 2_100_000_000,
        }]}
        result = collect_metrics(
            "run", "active", Path("/tmp"), [{"id": "ses_worker", "agent": "worker"}],
            lambda _session: messages, definition, records, now_ms=3000,
        )
        self.assertEqual(
            [warning["kind"] for warning in result["roles"][0]["warnings"]],
            ["artifact_pattern_no_match", "work_boundary_not_observed"],
        )

    def test_task_metrics_cover_tokens_thinking_and_declared_commands(self):
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
            lambda _session: messages, {"roles": {"a1": {
                "commands": {"telora": ["./bin/telora *"]},
            }}}, records, now_ms=3000,
        )
        task = result["tasks"][0]
        self.assertEqual(task["tokens"]["fresh"], 13)
        self.assertEqual(task["elapsed_ms"], 1200)
        self.assertEqual(task["longest_thinking_ms"], 600)
        self.assertEqual(task["commands"], {"telora": {"count": 1, "elapsed_ms": 100}})
        self.assertEqual(task["command_count"], 1)
        self.assertEqual(task["command_elapsed_ms"], 100)

    def test_declared_wrapper_command_is_counted_without_tool_special_case(self):
        messages = [{
            "info": {"role": "assistant", "time": {"created": 1000, "completed": 2000}},
            "parts": [{"type": "tool", "tool": "bash", "state": {
                "input": {"command": "just make-query"},
                "time": {"start": 1200, "end": 1500},
            }}],
        }]
        records = {"active": [], "history": [{
            "task_id": "a5-1", "role": "a5", "artifacts": ["answer.a5"],
            "status": "submitted", "started_at_ns": 900_000_000,
            "submitted_at_ns": 2_100_000_000,
        }]}
        result = collect_metrics(
            "run", "idle", Path("/tmp"), [{"id": "ses_a5", "agent": "a5"}],
            lambda _session: messages, {"roles": {"a5": {
                "commands": {"query": ["just make-query"]},
            }}}, records, now_ms=3000,
        )
        self.assertEqual(result["tasks"][0]["commands"], {
            "query": {"count": 1, "elapsed_ms": 300},
        })

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


    def test_replacement_sessions_are_aggregated_as_one_role(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            messages = {
                "old": [{"info": {"role": "assistant", "time": {"created": 1, "completed": 2},
                                    "tokens": {"input": 3}}, "parts": []}],
                "new": [{"info": {"role": "assistant", "time": {"created": 3, "completed": 4},
                                    "tokens": {"input": 5}}, "parts": []}],
            }
            result = collect_metrics(
                "run", "idle", workspace,
                [{"id": "old", "agent": "a5"}, {"id": "new", "agent": "a5"}],
                messages.__getitem__, {"roles": {}},
            )
            self.assertEqual(len(result["roles"]), 1)
            self.assertEqual(result["roles"][0]["session_ids"], ["old", "new"])
            self.assertEqual(result["roles"][0]["tokens"]["fresh"], 8)


class ArchiveExportTest(unittest.TestCase):
    def test_archive_is_repeatable_allows_internal_file_links_and_rejects_escape(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary); root = repo / "role"; workspace = repo / "workspace"
            (workspace / "output").mkdir(parents=True); (workspace / "output" / "x").write_text("x")
            manifest = Manifest("demo", repo, (), {}, (), ("output",), ("output",), ())
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
        return Manifest("demo", root, (), {"worker": {"preflight": list(commands)}},
                        (), (), (), ())

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

    def test_rejects_allowed_command_family_missing_from_preflight(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            workspace = self.workspace(root, {"bash": {
                "*": "deny",
                "./bin/telora query *": "allow",
                "./bin/telora types *": "allow",
            }})
            manifest = self.manifest(
                root, ("./bin/telora query exports @bin/main -C demo",)
            )
            with self.assertRaisesRegex(ControlError, "unexercised worker command family"):
                preflight_permissions(manifest, workspace)


class StatusSummaryTest(unittest.TestCase):
    def test_status_surfaces_host_gate_without_verbose_graph(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            workflow = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1"],
                "start_artifacts": ["lang"],
                "finish_artifact": "accepted",
                "artifacts": {
                    "lang": {"desc": "language"},
                    "draft.a1": {
                        "desc": "draft", "input": ["lang"], "instruction": "build",
                    },
                    "accepted": {"desc": "accepted", "input": ["draft.a1"]},
                },
            })
            publish_artifact(workspace, workflow, "lang")
            pull(workspace, workflow, "a1", False, None)
            submit(workspace, workflow, "a1", ["draft.a1"])
            context = mock.Mock(state={
                "exec_name": "demo", "phase": "active", "workspace": str(workspace),
                "workflow": workflow,
            })
            context.root = workspace / "execution"
            metrics = {"aggregate": {"tokens": {"fresh": 10}}}
            detail = {"agents": [{"role": "a1", "state": "waiting_on_pull"}],
                      "records": {"active": [], "history": []}}
            with mock.patch("tools.opencode_experiment.cli_ctl._metrics",
                            return_value=(metrics, detail)):
                summary = _status(context)
                verbose = _status(context, True)
            self.assertNotIn("artifacts", summary)
            self.assertEqual(summary["artifact_summary"]["publishable"], ["accepted"])
            self.assertEqual(summary["next_host_actions"], [{
                "action": "review_and_publish",
                "artifact": "accepted",
                "command": "oc-ctl publish demo accepted",
            }])
            self.assertIn("artifacts", verbose)

    def test_host_pull_returns_immediately_for_publishable_gate_and_summarizes_window(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            workflow = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1"],
                "start_artifacts": ["lang"],
                "finish_artifact": "accepted",
                "artifacts": {
                    "lang": {"desc": "language"},
                    "draft.a1": {"desc": "draft", "input": ["lang"], "instruction": "build"},
                    "accepted": {"desc": "accepted", "input": ["draft.a1"],
                                 "checks": ["approval.txt"]},
                },
            })
            publish_artifact(workspace, workflow, "lang")
            pulled = pull(workspace, workflow, "a1", False, None)
            submit(workspace, workflow, "a1", ["draft.a1"])
            context = mock.Mock(state={
                "exec_name": "demo", "phase": "active", "workspace": str(workspace),
                "workflow": workflow,
            })
            context.root = workspace / "execution"
            context.client.return_value.children.return_value = []
            result = _host_pull(context, pulled["started_at_ns"] // 1_000_000 - 1, timeout=60)
            self.assertEqual(result["reason"], "requests_changed")
            self.assertLess(result["waited_ms"], 1000)
            self.assertEqual(result["requests"], ["accepted"])
            self.assertFalse(workflow_status(workspace, workflow)["artifacts"]["accepted"]["publishable"])
            self.assertEqual([event["status"] for event in result["events"]
                              if event["type"] == "task"], ["submitted"])
            repeated = _host_pull(context, result["next_since"], timeout=0)
            self.assertEqual([event["id"] for event in repeated["events"]],
                             [event["id"] for event in result["events"]
                              if event["at"] == result["next_since"]])
            self.assertEqual(repeated["requests"], ["accepted"])

    def test_host_pull_reports_optional_requests_without_waking(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            workflow = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1"],
                "start_artifacts": ["lang"],
                "finish_artifact": "accepted",
                "artifacts": {
                    "lang": {"desc": "language"},
                    "feedback": {"desc": "optional Host feedback"},
                    "draft.a1": {"desc": "draft", "input": ["lang", "feedback?"],
                                 "instruction": "build"},
                    "accepted": {"desc": "accepted", "input": ["draft.a1"]},
                },
            })
            publish_artifact(workspace, workflow, "lang")
            context = mock.Mock(state={
                "exec_name": "demo", "phase": "active", "workspace": str(workspace),
                "workflow": workflow,
            })
            context.root = workspace / "execution"
            context.client.return_value.children.return_value = []

            result = _host_pull(context, None, timeout=.02)

            self.assertEqual(result["reason"], "timeout")
            self.assertGreaterEqual(result["waited_ms"], 10)
            self.assertEqual(result["requests"], [])
            self.assertEqual(result["opt_requests"], ["feedback"])

    def test_host_pull_rejects_waits_longer_than_one_minute(self):
        context = mock.Mock(state={"workflow": {"roles": []}})
        with self.assertRaisesRegex(ControlError, "between 0 and 60"):
            _host_pull(context, None, timeout=61)


class EventProjectionTest(unittest.TestCase):
    def test_projects_compact_lifecycle_events_and_reads_sanitized_detail(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            context = mock.Mock(state={"workspace": str(workspace)})
            context.root = workspace / "execution"
            message = {
                "info": {"id": "msg_1", "role": "assistant", "finish": "stop",
                         "time": {"created": 1000, "completed": 3600},
                         "tokens": {"input": 2, "output": 3, "reasoning": 5}},
                "parts": [
                    {"type": "reasoning", "text": "private reasoning"},
                    {"id": "part_1", "type": "tool", "tool": "bash", "state": {
                        "status": "completed", "input": {"command": "just make-query"},
                        "output": "ok", "metadata": {"exit": 0},
                        "time": {"start": 2200, "end": 2400},
                    }},
                    {"type": "text", "text": "Query completed."},
                ],
            }
            client = context.client.return_value
            client.children.return_value = [{"id": "ses_1", "agent": "a5"}]
            client.session_messages.return_value = [message]
            events = project_events(context, 999)
            self.assertEqual([event["type"] for event in events],
                             ["thinking", "action", "thinking", "reply"])
            self.assertEqual(events[1]["summary"], "just make-query")
            thinking = event_detail(context, "thinking:ses_1:msg_1:0")
            self.assertNotIn("parts", thinking["detail"])
            self.assertNotIn("private reasoning", json.dumps(thinking))
            self.assertEqual(thinking["detail"]["event"]["end_at"], 2200)
            action = event_detail(context, "action:ses_1:msg_1:part_1")
            self.assertEqual(action["detail"]["state"]["output"], "ok")


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
            manifest = Manifest("demo", root, (), {}, (), (), (), ())
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
        manifest = Manifest("demo", root, (), {}, (), (), (), ())
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
