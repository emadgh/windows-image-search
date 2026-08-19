from pathlib import Path


def patch_file(path: str, replacements):
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    for old, new in replacements:
        if new in text:
            continue
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected one match, found {count}: {old[:100]!r}")
        text = text.replace(old, new, 1)
    p.write_text(text, encoding="utf-8")


patch_file(
    "src/main.rs",
    [(
        "mod metadata;\nmod settings;",
        "mod metadata;\nmod material_texture;\nmod settings;",
    )],
)

patch_file(
    "src/db.rs",
    [
        (
            "use anyhow::{bail, Context, Result};",
            "use crate::material_texture;\nuse anyhow::{bail, Context, Result};",
        ),
        (
            "    pub color_histogram: Option<Vec<f32>>,\n    pub embedding: Option<Vec<f32>>,\n",
            "    pub color_histogram: Option<Vec<f32>>,\n    pub material_texture: Option<Vec<f32>>,\n    pub embedding: Option<Vec<f32>>,\n",
        ),
        (
            "            color_histogram BLOB,\n            color_histogram_dim INTEGER,\n            embedding BLOB,",
            "            color_histogram BLOB,\n            color_histogram_dim INTEGER,\n            material_texture BLOB,\n            material_texture_dim INTEGER,\n            material_texture_version INTEGER NOT NULL DEFAULT 0,\n            embedding BLOB,",
        ),
        (
            "    ensure_column(&conn, \"images\", \"color_histogram_dim\", \"INTEGER\")?;\n    ensure_column(\n        &conn,\n        \"images\",\n        \"embedding_normalized\",",
            "    ensure_column(&conn, \"images\", \"color_histogram_dim\", \"INTEGER\")?;\n    ensure_column(&conn, \"images\", \"material_texture\", \"BLOB\")?;\n    ensure_column(&conn, \"images\", \"material_texture_dim\", \"INTEGER\")?;\n    ensure_column(\n        &conn,\n        \"images\",\n        \"material_texture_version\",\n        \"INTEGER NOT NULL DEFAULT 0\",\n    )?;\n    ensure_column(\n        &conn,\n        \"images\",\n        \"embedding_normalized\",",
        ),
        (
            "            color_histogram_dim = excluded.color_histogram_dim,\n            embedding = NULL,",
            "            color_histogram_dim = excluded.color_histogram_dim,\n            material_texture = NULL,\n            material_texture_dim = NULL,\n            material_texture_version = 0,\n            embedding = NULL,",
        ),
        (
            "pub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {\n    let mut stmt = conn.prepare(\n        \"SELECT path FROM images WHERE visual_hash IS NULL OR color_histogram IS NULL ORDER BY path\",\n    )?;\n    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;\n    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())\n}",
            "pub fn set_material_texture(\n    conn: &Connection,\n    path: &Path,\n    descriptor: &[f32],\n) -> Result<()> {\n    conn.execute(\n        r#\"\n        UPDATE images\n        SET material_texture = ?2, material_texture_dim = ?3, material_texture_version = ?4\n        WHERE path = ?1\n        \"#,\n        params![\n            path.to_string_lossy().to_string(),\n            encode_f32_vec(descriptor),\n            descriptor.len() as i64,\n            material_texture::VERSION,\n        ],\n    )?;\n    Ok(())\n}\n\npub fn paths_missing_visual_descriptor(conn: &Connection) -> Result<Vec<PathBuf>> {\n    let mut stmt = conn.prepare(\n        \"SELECT path FROM images WHERE visual_hash IS NULL OR color_histogram IS NULL OR material_texture IS NULL OR material_texture_version <> ?1 ORDER BY path\",\n    )?;\n    let rows = stmt.query_map(params![material_texture::VERSION], |row| row.get::<_, String>(0))?;\n    Ok(rows.filter_map(|r| r.ok()).map(PathBuf::from).collect())\n}",
        ),
        (
            "               embedding, embedding_dim, embedding_normalized, rowid\n        FROM images",
            "               embedding, embedding_dim, embedding_normalized, rowid,\n               material_texture, material_texture_dim, material_texture_version\n        FROM images",
        ),
        (
            "        let embedding_normalized = row.get::<_, bool>(18)?;\n        Ok(ImageRecord {\n            rowid: row.get::<_, i64>(19)?.max(0) as usize,",
            "        let embedding_normalized = row.get::<_, bool>(18)?;\n        let material_texture_blob: Option<Vec<u8>> = row.get(20)?;\n        let material_texture_dim: Option<i64> = row.get(21)?;\n        let material_texture_version = row.get::<_, i64>(22)?;\n        let material_texture = if material_texture_version == material_texture::VERSION {\n            material_texture_blob.and_then(|bytes| {\n                decode_f32_vec(&bytes, material_texture_dim.unwrap_or(0).max(0) as usize)\n            })\n        } else {\n            None\n        };\n        Ok(ImageRecord {\n            rowid: row.get::<_, i64>(19)?.max(0) as usize,",
        ),
        (
            "            color_histogram,\n            embedding,\n            embedding_normalized,",
            "            color_histogram,\n            material_texture,\n            embedding,\n            embedding_normalized,",
        ),
        (
            "               embedding_normalized, rowid\n        FROM images\n        ORDER BY file_name COLLATE NOCASE",
            "               embedding_normalized, rowid,\n               material_texture, material_texture_dim, material_texture_version\n        FROM images\n        ORDER BY file_name COLLATE NOCASE",
        ),
        (
            "        let visual_hash_signed: Option<i64> = row.get(13)?;\n        Ok(ImageRecord {\n            rowid: row.get::<_, i64>(17)?.max(0) as usize,",
            "        let visual_hash_signed: Option<i64> = row.get(13)?;\n        let material_texture_blob: Option<Vec<u8>> = row.get(18)?;\n        let material_texture_dim: Option<i64> = row.get(19)?;\n        let material_texture_version = row.get::<_, i64>(20)?;\n        let material_texture = if material_texture_version == material_texture::VERSION {\n            material_texture_blob.and_then(|bytes| {\n                decode_f32_vec(&bytes, material_texture_dim.unwrap_or(0).max(0) as usize)\n            })\n        } else {\n            None\n        };\n        Ok(ImageRecord {\n            rowid: row.get::<_, i64>(17)?.max(0) as usize,",
        ),
        (
            "            color_histogram,\n            embedding: None,\n            embedding_normalized: row.get::<_, bool>(16)?,",
            "            color_histogram,\n            material_texture,\n            embedding: None,\n            embedding_normalized: row.get::<_, bool>(16)?,",
        ),
    ],
)

patch_file(
    "src/indexer.rs",
    [
        (
            "use crate::metadata;\nuse crate::settings::IndexingSettings;",
            "use crate::metadata;\nuse crate::material_texture;\nuse crate::settings::IndexingSettings;",
        ),
        (
            "    visual_hash: u64,\n    color_histogram: Vec<f32>,\n}",
            "    visual_hash: u64,\n    color_histogram: Vec<f32>,\n    material_texture: Vec<f32>,\n}",
        ),
        (
            "|(width, height, dominant, visual_hash, color_histogram)| {",
            "|(width, height, dominant, visual_hash, color_histogram, material_texture)| {",
        ),
        (
            "                                visual_hash,\n                                color_histogram,\n                            }",
            "                                visual_hash,\n                                color_histogram,\n                                material_texture,\n                            }",
        ),
        (
            "                    &item.color_histogram,\n                )?;\n                committed_paths.push(item.path.clone());",
            "                    &item.color_histogram,\n                )?;\n                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;\n                committed_paths.push(item.path.clone());",
        ),
        (
            "|(width, height, dominant, visual_hash, color_histogram)| {",
            "|(width, height, dominant, visual_hash, color_histogram, material_texture)| {",
        ),
        (
            "                                visual_hash,\n                                color_histogram,\n                            }",
            "                                visual_hash,\n                                color_histogram,\n                                material_texture,\n                            }",
        ),
        (
            "                    &item.color_histogram,\n                )?;\n            }\n            transaction.commit()?;",
            "                    &item.color_histogram,\n                )?;\n                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;\n            }\n            transaction.commit()?;",
        ),
        (
            "    let descriptors: Vec<(PathBuf, u64, Vec<f32>)> = pool.install(|| {",
            "    let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>)> = pool.install(|| {",
        ),
        (
            "                    Ok((_, visual_hash, color_histogram)) => {\n                        Some((path.clone(), visual_hash, color_histogram))\n                    }",
            "                    Ok((_, visual_hash, color_histogram, material_texture)) => {\n                        Some((path.clone(), visual_hash, color_histogram, material_texture))\n                    }",
        ),
        (
            "    for (path, visual_hash, color_histogram) in descriptors {\n        db::set_visual_descriptor(conn, &path, visual_hash, &color_histogram)?;\n    }",
            "    for (path, visual_hash, color_histogram, material_texture) in descriptors {\n        db::set_visual_descriptor(conn, &path, visual_hash, &color_histogram)?;\n        db::set_material_texture(conn, &path, &material_texture)?;\n    }",
        ),
        (
            "    let (query_dominant, query_hash, query_histogram) = visual_descriptor(&query_image);",
            "    let (query_dominant, query_hash, query_histogram, query_material_texture) =\n        visual_descriptor(&query_image);",
        ),
        (
            "        let hash_similarity = if compute_hash {\n            record\n                .visual_hash\n                .map(|hash| perceptual_hash_similarity(query_hash, hash))\n        } else {\n            None\n        };",
            "        let hash_similarity = if compute_hash {\n            let dhash = record\n                .visual_hash\n                .map(|hash| perceptual_hash_similarity(query_hash, hash));\n            let material = record.material_texture.as_deref().and_then(|descriptor| {\n                material_texture::similarity(&query_material_texture, descriptor)\n            });\n            material_texture::combine_with_dhash(dhash, material)\n        } else {\n            None\n        };",
        ),
        (
            "fn inspect_image(\n    path: &Path,\n    thumbnail_cache_dir: &Path,\n) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>)> {",
            "fn inspect_image(\n    path: &Path,\n    thumbnail_cache_dir: &Path,\n) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>, Vec<f32>)> {",
        ),
        (
            "    let (dominant, visual_hash, color_histogram) = visual_descriptor(&image);\n    Ok((width, height, dominant, visual_hash, color_histogram))",
            "    let (dominant, visual_hash, color_histogram, material_texture) =\n        visual_descriptor(&image);\n    Ok((\n        width,\n        height,\n        dominant,\n        visual_hash,\n        color_histogram,\n        material_texture,\n    ))",
        ),
        (
            "fn visual_descriptor(image: &DynamicImage) -> ([u8; 3], u64, Vec<f32>) {",
            "fn visual_descriptor(image: &DynamicImage) -> ([u8; 3], u64, Vec<f32>, Vec<f32>) {",
        ),
        (
            "    (dominant, visual_hash, color_histogram)\n}",
            "    let material_texture = material_texture::descriptor(image);\n    (dominant, visual_hash, color_histogram, material_texture)\n}",
        ),
    ],
)
