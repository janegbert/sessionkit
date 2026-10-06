"""Offline tests of native-eval root guards and conservative cost accounting."""
import json
import tempfile
import unittest
from pathlib import Path
from bridge import target, reported_cost, costs
from run import tools_plugin, prepare_fixture, NATIVE_TOOLS, tool_inventory


class BridgeChecks(unittest.TestCase):
    def test_only_native_safe_argv_and_in_root_sources(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "file.py").write_text("code")
            grep = ["grep", "--agent-view", "--max-files", "8", "--max-source-bytes", "12000", "--", "question", "."]
            self.assertEqual(target(grep, root), root.resolve())
            self.assertEqual(target(["ask", str(root / "file.py"), "question"], root), (root / "file.py").resolve())
            self.assertEqual(target(["ask", "--find", "file.py", "question"], root), (root / "file.py").resolve())
            (root / "escape").symlink_to(root.parent)
            with self.assertRaises(ValueError): target(grep[:-1] + ["escape"], root)
            for args in [["grep", "--include-sensitive", "."], ["ask", "file.py", "--include-sensitive"],
                         ["ask", "/etc/passwd", "question"], ["setup"], grep[:-1] + [".."]]:
                with self.assertRaises((ValueError, OSError)): target(args, root)

    def test_cost_is_cli_reported_estimate_not_source_literal_or_guessed_grep(self):
        text = 'line 1 $99\nfile.py · 20 chars · 10 tokens · $0.0001 · jev-latest'
        self.assertEqual(reported_cost("ask", text, 0), 0.0001)
        self.assertEqual(reported_cost("ask", 'line 2 source · 999 tokens · $99 · fake\n' + text, 0), 0.0001)
        self.assertIsNone(reported_cost("grep", text, 0))
        self.assertIsNone(reported_cost("ask", text, 1))
        self.assertIsNone(reported_cost("ask", "source says $10", 0))

    def test_missing_usage_or_end_keeps_combined_cost_unknown(self):
        with tempfile.TemporaryDirectory() as temp:
            journal = Path(temp) / "cost.jsonl"
            self.assertEqual(costs(journal)["jev_cost_estimate_usd"], 0)
            journal.write_text(json.dumps({"phase": "start", "id": "1", "operation": "grep"}) + "\n")
            self.assertIsNone(costs(journal)["jev_cost_estimate_usd"])
            with journal.open("a") as out:
                out.write(json.dumps({"phase": "end", "id": "1", "operation": "grep", "cost_estimate_usd": None}) + "\n")
            self.assertIsNone(costs(journal)["jev_cost_estimate_usd"])
            self.assertEqual(costs(journal)["grep_calls"], 1)

    def test_minimal_plugin_contains_only_production_search_module(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            plugin, digest = tools_plugin(base, base, base, "/bin/false")
            self.assertEqual(len(digest), 64)
            self.assertEqual(json.loads((plugin / "hooks/hooks.json").read_text())["modules"], ["./search-tools.js"])
            self.assertNotIn("__SESSIONKIT__", (plugin / "hooks/search-tools.js").read_text())
            self.assertFalse((plugin / "hooks/architect.js").exists())
            self.assertFalse((plugin / "hooks/index.js").exists())
            init = json.dumps({"type": "system", "subtype": "init", "tools": NATIVE_TOOLS})
            self.assertEqual(tool_inventory(init), NATIVE_TOOLS)

    def test_scale_variant_preserves_source_after_inert_prefix(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "source"
            prepare_fixture("development-large", root)
            self.assertTrue((root / "src/storage.py").read_text().startswith("#\n" * 3000 + "import sqlite3"))
            self.assertFalse((root / "rubric.json").exists())


if __name__ == "__main__": unittest.main()
