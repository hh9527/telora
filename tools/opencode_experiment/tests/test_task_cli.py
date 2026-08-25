from __future__ import annotations

import json
import tempfile
import threading
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

from tools.opencode_experiment.task_cli import (
    TaskError,
    evaluate,
    load_workflow,
    main,
    parser,
    publish_artifact,
    pull,
    remove_artifact,
    restore_artifacts,
    submit,
    task_records,
    validate_workflow,
    workflow_status,
)


def artifact_workflow() -> dict:
    return validate_workflow({
        "schema": "telora.artifact-workflow/v1",
        "roles": ["a1", "a2"],
        "start_artifacts": ["lang"],
        "finish_artifact": "qb",
        "artifacts": {
            "lang": {"desc": "语言输入", "checks": ["GOAL.md"]},
            "qb-feedback": {"desc": "Host 反馈", "checks": ["FEEDBACK.md"]},
            "qb.a1": {
                "desc": "A1 候选",
                "input": ["lang", "qb-feedback?"],
                "checks": ["output.txt"],
                "instruction": "生成 output.txt",
            },
            "qb-feedback.a2": {
                "desc": "A2 检视",
                "input": ["qb.a1"],
                "checks": ["review.txt"],
                "instruction": "检视 output.txt 并生成 review.txt",
            },
            "qb": {"desc": "Host 批准的候选", "input": ["qb.a1", "qb-feedback.a2"]},
        },
    })


class ArtifactWorkflowTest(unittest.TestCase):
    def prepare(self, root: Path) -> dict:
        (root / "GOAL.md").write_text("write one line", encoding="utf-8")
        value = artifact_workflow()
        (root / "experiment.json").write_text(json.dumps({"workflow": value}), encoding="utf-8")
        return value

    def test_optional_artifact_rebuild_and_restart_are_mtime_derived(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            self.assertEqual(load_workflow(root), value)
            publish_artifact(root, value, "lang")

            pulled = pull(root, value, "a1", False, None)
            self.assertEqual([item["id"] for item in pulled["artifacts"]], ["qb.a1"])
            output = pulled["artifacts"][0]
            self.assertEqual(output["output_mtime_ns"], 0)
            self.assertGreater(output["inputs"][0]["mtime_ns"], 0)
            self.assertTrue(output["inputs"][0]["changed"])
            self.assertFalse(output["inputs"][1]["available"])
            self.assertEqual(output["inputs"][1]["mtime_ns"], 0)
            self.assertFalse(output["inputs"][1]["changed"])
            (root / "output.txt").write_text("draft", encoding="utf-8")
            submit(root, value, "a1", ["qb.a1"])

            (root / "review.txt").write_text("review", encoding="utf-8")
            pull(root, value, "a2", False, None)
            submit(root, value, "a2", ["qb-feedback.a2"])
            (root / "FEEDBACK.md").write_text("revise", encoding="utf-8")
            publish_artifact(root, value, "qb-feedback")

            status = evaluate(root, value)
            self.assertTrue(status["artifacts"]["qb.a1"]["runnable"])
            self.assertEqual(status["artifacts"]["qb-feedback.a2"]["blocked_by"], ["qb.a1"])
            self.assertEqual(workflow_status(root, load_workflow(root)), status)

            (root / "output.txt").write_text("revised", encoding="utf-8")
            rebuilt = pull(root, value, "a1", False, None)["artifacts"][0]
            self.assertGreater(rebuilt["output_mtime_ns"], 0)
            self.assertFalse(rebuilt["inputs"][0]["changed"])
            self.assertTrue(rebuilt["inputs"][1]["changed"])
            submit(root, value, "a1", ["qb.a1"])
            (root / "review.txt").write_text("reviewed again", encoding="utf-8")
            pull(root, value, "a2", False, None)
            submit(root, value, "a2", ["qb-feedback.a2"])
            publish_artifact(root, value, "qb")
            self.assertTrue(evaluate(root, value)["quiescent"])

    def test_pull_timeout_explains_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            result = pull(root, value, "a2", True, 0)
            self.assertTrue(result["waiting"])
            self.assertEqual(result["reason"], "waiting for artifact inputs")
            self.assertEqual(result["waiting_for"], [{
                "artifact": "qb-feedback.a2",
                "blocked_by": ["qb.a1"],
            }])

    def test_pull_without_timeout_blocks_until_work_is_runnable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            result = []
            worker = threading.Thread(
                target=lambda: result.append(pull(root, value, "a1", True, None)),
                daemon=True,
            )
            worker.start()
            time.sleep(.05)
            self.assertTrue(worker.is_alive())
            publish_artifact(root, value, "lang")
            worker.join(2)
            self.assertFalse(worker.is_alive())
            self.assertEqual(result[0]["artifacts"][0]["id"], "qb.a1")

    def test_pull_returns_one_runnable_artifact_in_declaration_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = validate_workflow({
                "schema": "telora.artifact-workflow/v1",
                "roles": ["a1"],
                "start_artifacts": ["lang"],
                "finish_artifact": "finish",
                "artifacts": {
                    "lang": {"desc": "input", "checks": ["GOAL.md"]},
                    "first.a1": {
                        "desc": "first", "input": ["lang"],
                        "checks": ["first.txt"], "instruction": "write first",
                    },
                    "second.a1": {
                        "desc": "second", "input": ["lang"],
                        "checks": ["second.txt"], "instruction": "write second",
                    },
                    "finish": {"desc": "finish", "input": ["first.a1", "second.a1"]},
                },
            })
            (root / "GOAL.md").write_text("goal", encoding="utf-8")
            publish_artifact(root, value, "lang")

            first = pull(root, value, "a1", False, None)
            self.assertEqual([item["id"] for item in first["artifacts"]], ["first.a1"])
            (root / "first.txt").write_text("first", encoding="utf-8")
            submit(root, value, "a1", ["first.a1"])

            second = pull(root, value, "a1", False, None)
            self.assertEqual([item["id"] for item in second["artifacts"]], ["second.a1"])

    def test_role_and_host_ownership_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "role-owned"):
                publish_artifact(root, value, "qb.a1")
            with self.assertRaisesRegex(TaskError, "no active pulled task"):
                submit(root, value, "a1", ["qb-feedback.a2"])
            with self.assertRaisesRegex(TaskError, "no active pulled task"):
                submit(root, value, "a2", ["qb"])

    def test_host_can_remove_only_host_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            publish_artifact(root, value, "lang")
            self.assertTrue(remove_artifact(root, value, "lang")["removed"])
            self.assertFalse(evaluate(root, value)["artifacts"]["lang"]["current"])
            with self.assertRaisesRegex(TaskError, "role-owned"):
                remove_artifact(root, value, "qb.a1")
            forced = remove_artifact(root, value, "qb.a1", force=True)
            self.assertTrue(forced["host_forced"])

    def test_host_can_force_publish_role_artifact_without_bypassing_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            publish_artifact(root, value, "lang")
            pull(root, value, "a1", False, None)
            with self.assertRaisesRegex(TaskError, "checks are incomplete"):
                publish_artifact(root, value, "qb.a1", force=True)
            (root / "output.txt").write_text("Host supplied", encoding="utf-8")
            result = publish_artifact(root, value, "qb.a1", force=True)
            self.assertTrue(result["host_forced"])
            self.assertTrue(evaluate(root, value)["artifacts"]["qb.a1"]["current"])
            records = task_records(root)
            self.assertEqual(records["active"], [])
            self.assertEqual(records["history"][0]["status"], "stale")

    def test_host_publish_does_not_touch_artifact_when_checks_fail(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "checks are incomplete"):
                publish_artifact(root, value, "qb-feedback")
            self.assertFalse((root / "control" / "artifacts" / "qb-feedback").exists())

    def test_trusted_artifacts_can_be_restored_without_task_history(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            (root / "output.txt").write_text("inherited", encoding="utf-8")
            restored = restore_artifacts(root, value, ["lang", "qb.a1"])
            self.assertEqual([item["artifact"] for item in restored], ["lang", "qb.a1"])
            self.assertTrue(evaluate(root, value)["artifacts"]["qb.a1"]["current"])
            self.assertEqual(task_records(root), {"active": [], "history": []})

    def test_submit_requires_runnable_complete_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "no active pulled task"):
                submit(root, value, "a1", ["qb.a1"])
            publish_artifact(root, value, "lang")
            pull(root, value, "a1", False, None)
            with self.assertRaisesRegex(TaskError, "checks are incomplete"):
                submit(root, value, "a1", ["qb.a1"])

    def test_changed_inputs_supersede_active_task(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            publish_artifact(root, value, "lang")
            first = pull(root, value, "a1", False, None)
            (root / "FEEDBACK.md").write_text("new feedback", encoding="utf-8")
            publish_artifact(root, value, "qb-feedback")
            second = pull(root, value, "a1", False, None)
            self.assertNotEqual(first["task_id"], second["task_id"])
            records = task_records(root)
            self.assertEqual(records["history"][0]["status"], "stale")
            self.assertEqual(records["active"][0]["task_id"], second["task_id"])

    def test_validation_rejects_unknown_optional_input_and_cycle(self):
        raw = {
            "schema": "telora.artifact-workflow/v1",
            "roles": ["a1"],
            "start_artifacts": ["start"],
            "finish_artifact": "finish",
            "artifacts": {
                "start": {"desc": "start"},
                "work.a1": {"desc": "work", "input": ["missing?"], "instruction": "work"},
                "finish": {"desc": "finish", "input": ["work.a1"]},
            },
        }
        with self.assertRaisesRegex(TaskError, "unknown input"):
            validate_workflow(raw)
        raw["artifacts"]["work.a1"]["input"] = ["finish"]
        with self.assertRaisesRegex(TaskError, "dependency cycle"):
            validate_workflow(raw)

    def test_cli_is_only_pull_submit_and_status(self):
        self.assertIsNone(parser().parse_args(["pull", "a1"]).timeout)
        self.assertEqual(parser().parse_args(["pull", "a1", "--timeout", "60"]).timeout, 60.0)
        self.assertEqual(parser().parse_args(["submit", "a1", "qb.a1"]).artifacts, ["qb.a1"])
        self.assertEqual(parser().parse_args(["status"]).command, "status")
        with redirect_stderr(StringIO()), self.assertRaises(SystemExit):
            parser().parse_args(["mark-done", "a1", "qb.a1"])

    def test_wait_is_a_successful_heartbeat(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare(root)
            with redirect_stdout(StringIO()):
                self.assertEqual(main(["--root", str(root), "pull", "a2", "--timeout", "0"]), 0)


if __name__ == "__main__":
    unittest.main()
