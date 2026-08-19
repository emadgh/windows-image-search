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
replace_once(
    "src/face_store.rs",
    '''    let current = conn\n        .query_row(\n            r#"\n            SELECT 1\n            FROM face_detection_state\n            WHERE image_path = ?1\n              AND detector_id = ?2\n              AND detector_version = ?3\n              AND schema_version = ?4\n              AND source_size = ?5\n              AND source_modified = ?6\n            LIMIT 1\n            "#,\n            params![\n                image_path.to_string_lossy().to_string(),\n                detector_id,\n                detector_version,\n                face_detection::SCHEMA_VERSION,\n                source_size as i64,\n                source_modified,\n            ],\n            |_| Ok(()),\n        )\n        .is_ok();\n    Ok(current)''',
    '''    let current = conn.query_row(\n        r#"\n        SELECT 1\n        FROM face_detection_state\n        WHERE image_path = ?1\n          AND detector_id = ?2\n          AND detector_version = ?3\n          AND schema_version = ?4\n          AND source_size = ?5\n          AND source_modified = ?6\n        LIMIT 1\n        "#,\n        params![\n            image_path.to_string_lossy().to_string(),\n            detector_id,\n            detector_version,\n            face_detection::SCHEMA_VERSION,\n            source_size as i64,\n            source_modified,\n        ],\n        |_| Ok(()),\n    );\n    match current {\n        Ok(()) => Ok(true),\n        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),\n        Err(err) => Err(err.into()),\n    }''',
)
