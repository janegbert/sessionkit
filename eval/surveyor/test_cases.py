"""Executable oracles for synthetic cases; no HTTP/service execution."""
import importlib
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).parent


def module(case, name):
    namespace = f"survey_test_{case}"
    if namespace not in sys.modules:
        package = types.ModuleType(namespace)
        package.__path__ = [str(HERE / "cases" / case / "app")]
        sys.modules[namespace] = package
    return importlib.import_module(f"{namespace}.{name}")


class SnapshotCase(unittest.TestCase):
    def test_pending_job_keeps_copied_title_after_retitle(self):
        store = module("snapshot", "store").MemoryStore()
        store.submit("a", "old")
        store.retitle("a", "new")
        sent = []
        self.assertTrue(store.tick(lambda payload: sent.append(payload) or True))
        self.assertEqual(sent, [{"key": "a", "title": "old"}])
        self.assertEqual(store.records["a"]["title"], "new")
        self.assertFalse(store.tick(lambda payload: self.fail("no pending job")))

    def test_negative_ack_retains_snapshot(self):
        store = module("snapshot", "store").MemoryStore()
        store.submit("a", "old")
        store.retitle("a", "new")
        self.assertFalse(store.tick(lambda payload: False))
        self.assertEqual(store.pending[0]["payload"]["title"], "old")

    def test_imported_remote_is_not_registered(self):
        factory = module("snapshot", "factory")
        self.assertIsNotNone(factory.RemoteStore)
        self.assertEqual(list(factory.BACKENDS), ["memory"])
        with self.assertRaises(KeyError): factory.create_store("remote")


class RevisionCase(unittest.TestCase):
    def test_pending_job_uses_current_revision_even_with_old_cache_entry(self):
        sent = []
        app = module("revision", "bootstrap").Application(lambda payload: sent.append(payload) or True)
        app.submit("a", "old")
        app.cache.get(("a", 1), lambda: app.repository.read("a"))
        app.retitle("a", "new")
        self.assertTrue(app.worker.step())
        self.assertEqual(sent[0]["title"], "new")
        self.assertEqual(sent[0]["revision"], 2)
        self.assertIn(("a", 1), app.cache.values)
        self.assertIn(("a", 2), app.cache.values)
        app.retitle("a", "later")
        self.assertFalse(app.worker.step())
        self.assertEqual(len(sent), 1)

    def test_negative_ack_retains_identifier_and_retry_can_publish_new_revision(self):
        sent = []
        app = module("revision", "bootstrap").Application(lambda payload: sent.append(payload) or False)
        app.submit("a", "old")
        self.assertFalse(app.worker.step())
        app.retitle("a", "new")
        self.assertFalse(app.worker.step())
        self.assertEqual([payload["title"] for payload in sent], ["old", "new"])
        self.assertEqual(app.pending, ["a"])

    def test_reusing_a_key_can_reuse_a_cached_revision_and_serve_old_payload(self):
        # Diagnostic discovered after the v2 model runs, not an extra gold item.
        sent = []
        app = module("revision", "bootstrap").Application(lambda payload: sent.append(payload) or True)
        app.submit("a", "first")
        app.retitle("a", "cached revision two")
        app.worker.step()
        app.submit("a", "replacement")
        app.retitle("a", "current revision two")
        app.worker.step()
        self.assertEqual(app.repository.read("a")["title"], "current revision two")
        self.assertEqual(sent[-1]["title"], "cached revision two")

    def test_configured_dynamic_sender_resolves_without_http_call(self):
        bootstrap = module("revision", "bootstrap")
        sender = bootstrap.load_sender("survey_test_revision.transport:send")
        self.assertTrue(callable(sender))
        self.assertEqual(sender.__module__, "survey_test_revision.transport")
        with patch.dict("os.environ", {"DELIVERY_IMPL": "other.module:send"}):
            self.assertEqual(module("revision", "config").delivery_spec(), "other.module:send")


class DevelopmentRegression(unittest.TestCase):
    def test_existing_sqlite_job_reads_renamed_value_but_completed_job_does_not(self):
        namespace = "survey_test_development"
        package = types.ModuleType(namespace)
        package.__path__ = [str(HERE / "fixture/src")]
        sys.modules[namespace] = package
        storage = importlib.import_module(namespace + ".storage")
        connections = []
        original_connect = storage.connect
        def tracked_connect():
            connection = original_connect()
            connections.append(connection)
            return connection
        with tempfile.TemporaryDirectory() as temp, patch.object(storage, "database_path", lambda: str(Path(temp) / "test.sqlite")), patch.object(storage, "connect", tracked_connect):
            try:
                storage.initialize()
                key = storage.insert_item("old")
                storage.rename_direct(key, "new")
                self.assertEqual(storage.next_job(), (key, "new"))
                storage.finish_job(key)
                storage.rename_direct(key, "later")
                self.assertIsNone(storage.next_job())
            finally:
                for connection in connections: connection.close()


if __name__ == "__main__": unittest.main()
