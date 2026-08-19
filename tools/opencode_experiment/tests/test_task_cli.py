from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from tools.opencode_experiment.task_cli import (
    TaskError,
    evaluate,
    load_workflow,
    main,
    parser,
    publish_artifact,
    pull,
    submit,
    validate_workflow,
    workflow_status,
)


def artifact_workflow() -> dict:
    return validate_workflow({
        "schema": "telora.opencode-artifact-workflow/v1",
        "roles": ["a1", "a2"],
        "start_artifacts": ["lang"],
        "finish_artifact": "qb",
        "stop_path": "control/STOP",
        "artifacts": {
            "lang": {"desc": "语言输入", "checks": ["GOAL.md"]},
            "qb-feedback": {"desc": "Host 反馈", "checks": ["FEEDBACK.md"]},
            "qb.a1": {
                "desc": "A1 候选",
                "input": ["lang", "qb-feedback?"],
                "checks": ["output.txt"],
                "instruction": "生成 output.txt",
            },
            "qb-review.a2": {
                "desc": "A2 检视",
                "input": ["qb.a1"],
                "checks": ["review.txt"],
                "instruction": "检视 output.txt 并生成 review.txt",
            },
            "qb": {"desc": "Host 批准的候选", "input": ["qb.a1", "qb-review.a2"]},
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
            self.assertFalse(pulled["artifacts"][0]["inputs"][1]["available"])
            (root / "output.txt").write_text("draft", encoding="utf-8")
            submit(root, value, "a1", ["qb.a1"])

            (root / "review.txt").write_text("review", encoding="utf-8")
            submit(root, value, "a2", ["qb-review.a2"])
            (root / "FEEDBACK.md").write_text("revise", encoding="utf-8")
            publish_artifact(root, value, "qb-feedback")

            status = evaluate(root, value)
            self.assertTrue(status["artifacts"]["qb.a1"]["runnable"])
            self.assertEqual(status["artifacts"]["qb-review.a2"]["blocked_by"], ["qb.a1"])
            self.assertEqual(workflow_status(root, load_workflow(root)), status)

            (root / "output.txt").write_text("revised", encoding="utf-8")
            submit(root, value, "a1", ["qb.a1"])
            (root / "review.txt").write_text("reviewed again", encoding="utf-8")
            submit(root, value, "a2", ["qb-review.a2"])
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
                "artifact": "qb-review.a2",
                "blocked_by": ["qb.a1"],
            }])

    def test_stop_file_releases_waiting_roles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            (root / "control").mkdir()
            (root / "control" / "STOP").touch()
            self.assertTrue(pull(root, value, "a1", False, None)["stopped"])

    def test_role_and_host_ownership_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "role-owned"):
                publish_artifact(root, value, "qb.a1")
            with self.assertRaisesRegex(TaskError, "not owned by a1"):
                submit(root, value, "a1", ["qb-review.a2"])
            with self.assertRaisesRegex(TaskError, "not owned by a2"):
                submit(root, value, "a2", ["qb"])

    def test_submit_requires_runnable_complete_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "not runnable"):
                submit(root, value, "a1", ["qb.a1"])
            publish_artifact(root, value, "lang")
            with self.assertRaisesRegex(TaskError, "checks are incomplete"):
                submit(root, value, "a1", ["qb.a1"])

    def test_validation_rejects_unknown_optional_input_and_cycle(self):
        raw = {
            "schema": "telora.opencode-artifact-workflow/v1",
            "roles": ["a1"],
            "start_artifacts": ["start"],
            "finish_artifact": "finish",
            "stop_path": "control/STOP",
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
        self.assertEqual(parser().parse_args(["pull", "a1"]).timeout, 60.0)
        self.assertEqual(parser().parse_args(["submit", "a1", "qb.a1"]).artifacts, ["qb.a1"])
        self.assertEqual(parser().parse_args(["status"]).command, "status")
        with self.assertRaises(SystemExit):
            parser().parse_args(["mark-done", "a1", "qb.a1"])

    def test_wait_is_a_successful_heartbeat(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare(root)
            self.assertEqual(main(["--root", str(root), "pull", "a2", "--timeout", "0"]), 0)


if __name__ == "__main__":
    unittest.main()
