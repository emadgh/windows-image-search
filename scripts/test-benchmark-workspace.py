"""Regression tests use only temporary toy databases, never application data."""
from contextlib import closing
import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "workspace", Path(__file__).with_name("prepare-benchmark-workspace.py")
)
workspace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workspace)


class WorkspaceTest(unittest.TestCase):
    def test_copy_filters_only_working_rows_and_audit_detects_changes(self):
        with tempfile.TemporaryDirectory(prefix="wis-workspace-test-") as directory:
            base = Path(directory)
            root = base / "photos"
            portable = root / ".imagesearch"
            portable.mkdir(parents=True)
            session = base / "session.sqlite3"
            for database in (session, portable / "index.sqlite3"):
                with closing(sqlite3.connect(database)) as conn, conn:
                    conn.executescript("""
                        CREATE TABLE images(path TEXT, root TEXT, embedding BLOB);
                        CREATE TABLE roots(path TEXT);
                        CREATE TABLE portable_root_registry(path TEXT);
                    """)
                    conn.executemany("INSERT INTO images VALUES(?,?,?)", [
                        ("one.jpg", str(root), b"vector"),
                        ("other.jpg", "another-collection", None),
                    ])
                    conn.execute("INSERT INTO roots VALUES(?)", (str(root),))
            destination = base / "benchmark"
            workspace.prepare(session, root, destination)
            with closing(sqlite3.connect(destination / "index.sqlite3")) as conn, conn:
                self.assertEqual(conn.execute("SELECT count(*) FROM images").fetchone()[0], 1)
                self.assertEqual(conn.execute("SELECT count(*) FROM roots").fetchone()[0], 0)
                conn.execute("UPDATE images SET embedding=?", (b"changed-copy",))
            self.assertTrue(all(x["unchanged"] for x in workspace.verify(destination).values()))
            with self.assertRaises(FileExistsError):
                workspace.prepare(session, root, destination)
            with closing(sqlite3.connect(session)) as conn, conn:
                self.assertEqual(conn.execute("SELECT count(*) FROM images").fetchone()[0], 2)
                conn.execute("UPDATE images SET embedding=?", (b"external-write",))
            self.assertFalse(workspace.verify(destination)["session"]["unchanged"])


if __name__ == "__main__":
    unittest.main()
