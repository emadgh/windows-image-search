from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:160]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.2.10"', 'version = "0.3.0-alpha.1"')
replace_once(
    "src/main.rs",
    "mod embedding;\nmod fs_watch;\n",
    "mod embedding;\nmod face_detection;\nmod face_store;\nmod fs_watch;\n",
)
replace_once(
    "src/portable.rs",
    "use crate::{ann, db, thumbnail_cache};\n",
    "use crate::{ann, db, face_store, thumbnail_cache};\n",
)
replace_once(
    "src/portable.rs",
    '''    let conn = db::open(&index_db_path(root))?;\n    conn.execute_batch(''',
    '''    let conn = db::open(&index_db_path(root))?;\n    face_store::ensure_schema(&conn)?;\n    conn.execute_batch(''',
)
