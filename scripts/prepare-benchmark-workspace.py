"""Create an isolated benchmark copy using SQLite's online backup API.

Original connections use mode=ro. Image paths remain read-only benchmark inputs;
database, ANN, preview, report and model-cache writes stay in the new workspace.
"""
from contextlib import closing
import argparse
import hashlib
import json
from pathlib import Path
import pickle
import shutil
import sqlite3
import sys

MARKER = "windows-image-search isolated benchmark workspace v1"


def audit(connection):
    digest = hashlib.sha256()
    counts = {}
    tables = connection.execute(
        "SELECT name,sql FROM sqlite_master WHERE type='table' "
        "AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'images_fts%' ORDER BY name"
    ).fetchall()
    for name, schema in tables:
        escaped = '"' + name.replace('"', '""') + '"'
        digest.update(pickle.dumps((name, schema), protocol=4))
        try:
            rows = connection.execute(f"SELECT * FROM {escaped} ORDER BY rowid")
        except sqlite3.OperationalError:
            rows = connection.execute(f"SELECT * FROM {escaped}")
        count = 0
        for row in rows:
            digest.update(pickle.dumps(row, protocol=4))
            count += 1
        counts[name] = count
    return {"logical_sha256": digest.hexdigest(), "table_counts": counts}


def readonly(path):
    return sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True)


def prepare(session, root, destination):
    destination.mkdir(parents=True, exist_ok=False)
    baseline = destination / "baseline"
    baseline.mkdir()
    sources = {"session": session, "portable": root / ".imagesearch" / "index.sqlite3"}
    evidence = {}
    for label, source in sources.items():
        backup_path = baseline / f"{label}.sqlite3"
        with closing(readonly(source)) as reader, closing(sqlite3.connect(backup_path)) as backup:
            reader.backup(backup)
            if backup.execute("PRAGMA quick_check").fetchone() != ("ok",):
                raise RuntimeError(f"{label} backup integrity check failed")
            evidence[label] = {"source": str(source), **audit(backup)}
    shutil.copy2(baseline / "session.sqlite3", destination / "index.sqlite3")
    with closing(sqlite3.connect(destination / "index.sqlite3")) as working, working:
        working.execute("DELETE FROM images WHERE root != ? COLLATE NOCASE", (str(root),))
        # A second barrier in addition to CLI isolation: no original roots can
        # be hydrated even by an older diagnostic binary that reads this copy.
        working.execute("DELETE FROM roots")
        working.execute("DELETE FROM portable_root_registry")
        working.commit()
        evidence["working_images"] = working.execute("SELECT count(*) FROM images").fetchone()[0]
        evidence["working_embeddings"] = working.execute("SELECT count(*) FROM images WHERE embedding IS NOT NULL").fetchone()[0]
    source_models = session.parent / "models"
    if source_models.is_dir():
        shutil.copytree(source_models, destination / "models", symlinks=False)
    else:
        (destination / "models").mkdir()
    (destination / ".wis-benchmark-copy").write_text(MARKER, encoding="utf-8")
    (destination / "source-audit.json").write_text(json.dumps(evidence, indent=2), encoding="utf-8")
    return evidence


def verify(destination):
    previous = json.loads((destination / "source-audit.json").read_text(encoding="utf-8"))
    result = {}
    for label in ("session", "portable"):
        with closing(readonly(Path(previous[label]["source"]))) as reader:
            reader.execute("BEGIN")
            current = audit(reader)
            current["quick_check"] = reader.execute("PRAGMA quick_check").fetchone()[0]
        result[label] = {"unchanged": current["logical_sha256"] == previous[label]["logical_sha256"], **current}
    (destination / "source-audit-after.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--session", type=Path)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    if args.verify:
        result = verify(args.destination)
        print(json.dumps(result, indent=2))
        if not all(item["unchanged"] and item["quick_check"] == "ok" for item in result.values()):
            sys.exit(2)
    else:
        if args.session is None or args.root is None:
            parser.error("--session and --root are required to prepare a new workspace")
        print(json.dumps(prepare(args.session, args.root, args.destination), indent=2))
