from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from tools.opencode_experiment.task_cli import (
    TaskError,
    evaluate,
    load_workflow,
    mark_done,
    next_task,
    publish_node,
    validate_workflow,
)


def workflow() -> dict:
    return validate_workflow({
        "schema": "telora.opencode-node-workflow/v1",
        "start_nodes": ["lang.ready"],
        "finish_node": "consume.ready",
        "stop_path": "control/STOP",
        "nodes": [
            {"id": "lang.ready", "checks": ["docs/GOAL.md"]},
            {"id": "build-feedback.feedback", "needs": ["build-review-a2.rc"],
             "observes": "build.rc"},
            {"id": "build.rc", "role": "a1", "needs": ["lang.ready"],
             "inputs": ["build-feedback.feedback"], "checks": ["output.txt"]},
            {"id": "build-review-a2.rc", "role": "a2", "needs": ["build.rc"],
             "checks": ["review.txt"]},
            {"id": "build.ready", "needs": ["build.rc"]},
            {"id": "consume.rc", "role": "a2", "needs": ["build.ready"],
             "checks": ["consumed.txt"]},
            {"id": "consume.ready", "needs": ["consume.rc"]},
        ],
        "tasks": [
            {"id": "build.rc", "role": "a1", "needs": ["lang.ready"],
             "inputs": [], "outputs": ["output.txt"], "instruction": "build output"},
            {"id": "build-review-a2.rc", "role": "a2", "needs": ["build.rc"],
             "inputs": ["output.txt"], "outputs": ["review.txt"], "instruction": "review output"},
            {"id": "consume.rc", "role": "a2", "needs": ["build.ready"],
             "absorbs": ["build-review-a2.rc"], "inputs": ["output.txt"],
             "outputs": ["consumed.txt"], "instruction": "consume output"},
        ],
    })


class TaskCliTest(unittest.TestCase):
    def prepare(self, root: Path) -> dict:
        (root / "docs").mkdir()
        (root / "docs" / "GOAL.md").write_text("goal", encoding="utf-8")
        value = workflow()
        (root / "experiment.json").write_text(json.dumps({"workflow": value}), encoding="utf-8")
        return value

    @staticmethod
    def complete_build(root: Path, value: dict, text: str = "ready") -> None:
        claim = next_task(root, value, "a1", False, None)
        assert claim["task"] == "build.rc"
        (root / "output.txt").write_text(text, encoding="utf-8")
        mark_done(root, value, "a1", "build.rc")

    def test_standalone_review_and_feedback_invalidate_candidate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            self.assertEqual(load_workflow(root), value)
            publish_node(root, value, "lang.ready")
            self.complete_build(root, value)

            claim = next_task(root, value, "a2", False, None)
            self.assertEqual(claim["task"], "build-review-a2.rc")
            self.assertEqual(claim["absorbed"], [])
            (root / "review.txt").write_text("review", encoding="utf-8")
            result = mark_done(root, value, "a2", "build-review-a2.rc")
            self.assertFalse(result["claim_retained"])

            feedback = publish_node(root, value, "build-feedback.feedback", b"fix the contract\n")
            self.assertGreater(feedback["mtime_ns"], evaluate(root, value)["nodes"]["build.rc"]["stamp_mtime_ns"])
            self.assertTrue(evaluate(root, value)["tasks"]["build.rc"]["runnable"])

    def test_build_absorbs_runnable_review_and_keeps_parent_claim(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            publish_node(root, value, "lang.ready")
            self.complete_build(root, value)
            publish_node(root, value, "build.ready")

            claim = next_task(root, value, "a2", False, None)
            self.assertEqual(claim["task"], "consume.rc")
            self.assertEqual([item["task"] for item in claim["absorbed"]], ["build-review-a2.rc"])
            with self.assertRaisesRegex(TaskError, "absorbed tasks must be completed"):
                mark_done(root, value, "a2", "consume.rc")

            (root / "review.txt").write_text("review", encoding="utf-8")
            result = mark_done(root, value, "a2", "build-review-a2.rc")
            self.assertTrue(result["claim_retained"])
            self.assertEqual(result["parent"], "consume.rc")
            resumed = next_task(root, value, "a2", False, None)
            self.assertEqual(resumed["task"], "consume.rc")
            self.assertEqual(resumed["absorbed"], [])

            (root / "consumed.txt").write_text("done", encoding="utf-8")
            result = mark_done(root, value, "a2", "consume.rc")
            self.assertFalse(result["claim_retained"])
            publish_node(root, value, "consume.ready")
            self.assertTrue(evaluate(root, value)["quiescent"])

    def test_completed_review_is_not_absorbed_again(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            publish_node(root, value, "lang.ready")
            self.complete_build(root, value)
            next_task(root, value, "a2", False, None)
            (root / "review.txt").write_text("review", encoding="utf-8")
            mark_done(root, value, "a2", "build-review-a2.rc")
            publish_node(root, value, "build.ready")
            claim = next_task(root, value, "a2", False, None)
            self.assertEqual(claim["task"], "consume.rc")
            self.assertEqual(claim["absorbed"], [])

    def test_node_ownership_and_explicit_rc_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            with self.assertRaisesRegex(TaskError, "Host-owned"):
                publish_node(root, value, "build.rc")
            with self.assertRaisesRegex(TaskError, "must end in .rc"):
                mark_done(root, value, "a1", "build")
            with self.assertRaisesRegex(TaskError, "stale node"):
                publish_node(root, value, "build-feedback.feedback", b"feedback")

    def test_stop_file_releases_waiting_roles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            (root / "control").mkdir()
            (root / "control" / "STOP").touch()
            self.assertTrue(next_task(root, value, "a1", False, None)["stopped"])

    def test_mark_done_rejects_changed_inputs_and_releases_claim(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); value = self.prepare(root)
            publish_node(root, value, "lang.ready")
            next_task(root, value, "a1", False, None)
            (root / "output.txt").write_text("ready", encoding="utf-8")
            publish_node(root, value, "lang.ready")
            with self.assertRaisesRegex(TaskError, "inputs changed"):
                mark_done(root, value, "a1", "build.rc")
            self.assertFalse(next_task(root, value, "a1", False, None)["resumed"])

    def test_rejects_invalid_task_names_ownership_and_absorption_cycles(self):
        value = {
            "schema": "telora.opencode-node-workflow/v1",
            "start_nodes": ["start.ready"], "finish_node": "finish.ready",
            "stop_path": "control/STOP",
            "nodes": [
                {"id": "start.ready"}, {"id": "a.rc", "role": "r"},
                {"id": "b.rc", "role": "r"}, {"id": "finish.ready", "needs": ["a.rc"]},
            ],
            "tasks": [
                {"id": "a.rc", "role": "r", "absorbs": ["b.rc"], "instruction": "a"},
                {"id": "b.rc", "role": "r", "absorbs": ["a.rc"], "instruction": "b"},
            ],
        }
        with self.assertRaisesRegex(TaskError, "absorption cycle"):
            validate_workflow(value)
        value["tasks"][1]["absorbs"] = []
        value["tasks"][1]["id"] = "b"
        with self.assertRaisesRegex(TaskError, "must end in .rc"):
            validate_workflow(value)


if __name__ == "__main__":
    unittest.main()
