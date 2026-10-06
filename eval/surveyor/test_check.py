"""Offline evaluator tests. No model or network calls."""
import copy
import tempfile
import unittest
from pathlib import Path
from check import validate
from run import parse_stream, schedule, fixture_path

HERE = Path(__file__).parent


def report():
    return {"scope": "static fixture", "claims": [{"id": "config", "text": "Source supports an environment override.",
        "standing": "observed", "evidence": [{"path": "src/settings.py", "start": 5, "end": 5,
        "quote": 'return os.environ.get("PROJECT_DB", "dev.sqlite")'}]}],
        "questions": [], "unknowns": ["No production observation"], "stop_reason": "scope complete"}


class CitationChecks(unittest.TestCase):
    def test_valid_quote_is_only_structural_validation(self):
        data = report()
        data["claims"][0]["text"] = "An intentionally unsupported interpretation"
        self.assertEqual(validate(data, HERE / "fixture"), [])

    def test_fabricated_quotes_bad_lines_and_paths_fail(self):
        for key, value in [("quote", "fabricated"), ("start", 0), ("start", True), ("end", 999),
                           ("path", "../rubric.json"), ("path", "/etc/passwd"), ("path", "missing.py")]:
            data = report()
            data["claims"][0]["evidence"][0][key] = value
            self.assertTrue(validate(data, HERE / "fixture"), (key, value))

    def test_symlink_escape_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / "root"
            root.mkdir()
            (base / "outside").write_text("outside")
            (root / "escape").symlink_to(base / "outside")
            data = report()
            data["claims"][0]["evidence"] = [{"path": "escape", "start": 1, "end": 1, "quote": "outside"}]
            self.assertTrue(validate(data, root))

    def test_standing_missing_evidence_and_duplicate_ids(self):
        for mutate in [lambda c: c.update(standing="confirmed"), lambda c: c.update(evidence=[])]:
            data = report()
            mutate(data["claims"][0])
            self.assertTrue(validate(data, HERE / "fixture"))
        data = report()
        data["claims"].append(copy.deepcopy(data["claims"][0]))
        self.assertTrue(validate(data, HERE / "fixture"))

    def test_unknown_may_have_no_source(self):
        data = report()
        data["claims"][0].update(standing="unknown", evidence=[])
        self.assertEqual(validate(data, HERE / "fixture"), [])

    def test_stream_requires_final_and_deduplicates_tool_ids(self):
        import json
        message = {"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "call1", "name": "Read"}]}}
        text = "\n".join(json.dumps(e) for e in [message, message, {"type": "result", "structured_output": report()}])
        result, tools, chars, plugins = parse_stream(text)
        self.assertEqual(list(tools.values()), ["Read"])
        self.assertEqual(result["structured_output"], report())
        self.assertEqual(chars, 0)
        self.assertEqual(plugins, [])
        tool_result = {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "call1", "content": "text"}]}}
        stream = "\n".join(json.dumps(e) for e in [tool_result, tool_result, {"type": "result"}])
        self.assertEqual(parse_stream(stream)[2], 4)
        with self.assertRaises(ValueError): parse_stream("not json\nnull\n0")


class ComparisonPlan(unittest.TestCase):
    def test_case_orders_are_counterbalanced_within_and_across_repetitions(self):
        planned = list(schedule(["snapshot", "revision"], 2, "both"))
        self.assertEqual(len(planned), 8)
        self.assertEqual([v for r, c, v in planned if c == "snapshot"], ["legacy", "survey", "survey", "legacy"])
        self.assertEqual([v for r, c, v in planned if c == "revision"], ["survey", "legacy", "legacy", "survey"])

    def test_only_named_synthetic_sources_can_be_selected(self):
        for name in ["development", "snapshot", "revision"]: self.assertTrue(fixture_path(name).is_dir())
        with self.assertRaises(ValueError): fixture_path("../rubric.json")


if __name__ == "__main__": unittest.main()
