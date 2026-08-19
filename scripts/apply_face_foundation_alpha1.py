from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:180]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


# Earlier validation already committed the alpha version/modules/schema hook.
# Apply only the correctness refinements still missing from the branch.
replace_once(
    "src/main.rs",
    "mod face_store;\nmod fs_watch;\n",
    "mod face_store;\n#[cfg(test)]\nmod face_portable_tests;\nmod fs_watch;\n",
)
replace_once(
    "src/face_store.rs",
    '''    let current = conn\n        .query_row(\n            r#"\n            SELECT 1\n            FROM face_detection_state\n            WHERE image_path = ?1\n              AND detector_id = ?2\n              AND detector_version = ?3\n              AND schema_version = ?4\n              AND source_size = ?5\n              AND source_modified = ?6\n            LIMIT 1\n            "#,\n            params![\n                image_path.to_string_lossy().to_string(),\n                detector_id,\n                detector_version,\n                face_detection::SCHEMA_VERSION,\n                source_size as i64,\n                source_modified,\n            ],\n            |_| Ok(()),\n        )\n        .is_ok();\n    Ok(current)''',
    '''    let current = conn.query_row(\n        r#"\n        SELECT 1\n        FROM face_detection_state\n        WHERE image_path = ?1\n          AND detector_id = ?2\n          AND detector_version = ?3\n          AND schema_version = ?4\n          AND source_size = ?5\n          AND source_modified = ?6\n        LIMIT 1\n        "#,\n        params![\n            image_path.to_string_lossy().to_string(),\n            detector_id,\n            detector_version,\n            face_detection::SCHEMA_VERSION,\n            source_size as i64,\n            source_modified,\n        ],\n        |_| Ok(()),\n    );\n    match current {\n        Ok(()) => Ok(true),\n        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),\n        Err(err) => Err(err.into()),\n    }''',
)
replace_once(
    "src/portable.rs",
    '''        let tx = conn.transaction()?;\n        tx.execute(&format!("DELETE FROM {ATTACHED_DB}.images"), [])?;\n        tx.execute(&format!("DELETE FROM {ATTACHED_DB}.roots"), [])?;\n        tx.execute(\n            &format!("INSERT OR IGNORE INTO {ATTACHED_DB}.roots(path) VALUES('.')"),\n            [],\n        )?;\n        let copied = tx.execute(\n            &format!(\n                "INSERT INTO {ATTACHED_DB}.images({}) \\\n                 SELECT CASE WHEN path = ?1 THEN '' ELSE substr(path, length(?2) + 1) END, '', {} \\\n                 FROM main.images WHERE root = ?1",\n                image_columns_with_path(),\n                image_columns_without_path_root()\n            ),\n            params![root_text, prefix],\n        )?;\n        set_attached_meta(&tx, "schema_version", &PORTABLE_SCHEMA_VERSION.to_string())?;''',
    '''        let tx = conn.transaction()?;\n        tx.execute(\n            &format!("INSERT OR IGNORE INTO {ATTACHED_DB}.roots(path) VALUES('.')"),\n            [],\n        )?;\n        let copied = tx.execute(\n            &format!(\n                "INSERT INTO {ATTACHED_DB}.images({}) \\\n                 SELECT CASE WHEN path = ?1 THEN '' ELSE substr(path, length(?2) + 1) END, '', {} \\\n                 FROM main.images WHERE root = ?1 \\\n                 ON CONFLICT(path) DO UPDATE SET {}",\n                image_columns_with_path(),\n                image_columns_without_path_root(),\n                image_update_assignments()\n            ),\n            params![root_text, prefix],\n        )?;\n        tx.execute(\n            &format!(\n                "DELETE FROM {ATTACHED_DB}.images \\\n                 WHERE path NOT IN (\\
                    SELECT CASE WHEN path = ?1 THEN '' ELSE substr(path, length(?2) + 1) END \\\n                    FROM main.images WHERE root = ?1\\
                 )"\n            ),\n            params![root_text, prefix],\n        )?;\n        set_attached_meta(&tx, "schema_version", &PORTABLE_SCHEMA_VERSION.to_string())?;''',
)
replace_once(
    "src/portable.rs",
    '''            tx.execute(\n                &format!("DELETE FROM {ATTACHED_DB}.images WHERE path = ?1"),\n                params![relative_text],\n            )?;\n            tx.execute(\n                &format!(\n                    "INSERT INTO {ATTACHED_DB}.images({}) \\\n                     SELECT ?1, '', {} FROM main.images WHERE path = ?2 AND root = ?3",\n                    image_columns_with_path(),\n                    image_columns_without_path_root()\n                ),\n                params![\n                    relative.to_string_lossy().to_string(),\n                    absolute_text,\n                    root_text\n                ],\n            )?;''',
    '''            tx.execute(\n                &format!(\n                    "INSERT INTO {ATTACHED_DB}.images({}) \\\n                     SELECT ?1, '', {} FROM main.images WHERE path = ?2 AND root = ?3 \\\n                     ON CONFLICT(path) DO UPDATE SET {}",\n                    image_columns_with_path(),\n                    image_columns_without_path_root(),\n                    image_update_assignments()\n                ),\n                params![relative_text, absolute_text, root_text],\n            )?;''',
)
replace_once(
    "src/portable.rs",
    '''fn image_columns_without_path_root() -> &'static str {\n    "file_name, extension, size, modified, width, height, description, keywords, \\\n     dominant_r, dominant_g, dominant_b, visual_hash, color_histogram, color_histogram_dim, \\\n     material_texture, material_texture_dim, material_texture_version, embedding, embedding_dim, \\\n     embedding_normalized, last_seen_scan, content_fingerprint"\n}\n''',
    '''fn image_columns_without_path_root() -> &'static str {\n    "file_name, extension, size, modified, width, height, description, keywords, \\\n     dominant_r, dominant_g, dominant_b, visual_hash, color_histogram, color_histogram_dim, \\\n     material_texture, material_texture_dim, material_texture_version, embedding, embedding_dim, \\\n     embedding_normalized, last_seen_scan, content_fingerprint"\n}\n\nfn image_update_assignments() -> &'static str {\n    "root = excluded.root, \\\n     file_name = excluded.file_name, extension = excluded.extension, \\\n     size = excluded.size, modified = excluded.modified, width = excluded.width, height = excluded.height, \\\n     description = excluded.description, keywords = excluded.keywords, \\\n     dominant_r = excluded.dominant_r, dominant_g = excluded.dominant_g, dominant_b = excluded.dominant_b, \\\n     visual_hash = excluded.visual_hash, color_histogram = excluded.color_histogram, \\\n     color_histogram_dim = excluded.color_histogram_dim, material_texture = excluded.material_texture, \\\n     material_texture_dim = excluded.material_texture_dim, \\\n     material_texture_version = excluded.material_texture_version, embedding = excluded.embedding, \\\n     embedding_dim = excluded.embedding_dim, embedding_normalized = excluded.embedding_normalized, \\\n     last_seen_scan = excluded.last_seen_scan, content_fingerprint = excluded.content_fingerprint"\n}\n''',
)
