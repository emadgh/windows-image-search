from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace(path: str, old: str, new: str, count: int = 1) -> None:
    text = read(path)
    actual = text.count(old)
    if actual < count:
        raise RuntimeError(f"{path}: expected at least {count} occurrence(s), found {actual}: {old[:120]!r}")
    text = text.replace(old, new, count)
    write(path, text)


# ---------------------------------------------------------------------------
# Replaceable contracts: stable model version + dynamic cache revision.
# ---------------------------------------------------------------------------
replace(
    "src/face_detection.rs",
    "    fn detector_version(&self) -> &'static str;\n    fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>>;",
    "    fn detector_version(&self) -> &'static str;\n\n    fn cache_revision(&self) -> String {\n        self.detector_version().to_owned()\n    }\n\n    fn detect(&mut self, image: &DynamicImage) -> Result<Vec<DetectedFace>>;",
)

replace(
    "src/face_embedding.rs",
    "    fn model_version(&self) -> &'static str;\n    fn input_size(&self) -> u32;",
    "    fn model_version(&self) -> &'static str;\n\n    fn cache_revision(&self) -> String {\n        self.model_version().to_owned()\n    }\n\n    fn input_size(&self) -> u32;",
)

# ---------------------------------------------------------------------------
# YuNet production adapter: no leaked strings, provider excluded from semantic
# cache identity, stable MODEL_VERSION remains persisted separately.
# ---------------------------------------------------------------------------
replace(
    "src/face_detection/yunet_production.rs",
    "    cache_revision: &'static str,",
    "    cache_revision: String,",
)
replace(
    "src/face_detection/yunet_production.rs",
    "        let cache_revision = detector_cache_revision(\n            adapter.model_fingerprint(),\n            settings.score_threshold,\n            settings.nms_threshold,\n            settings.top_k,\n        );\n        let cache_revision = Box::leak(cache_revision.into_boxed_str());",
    "        let cache_revision = detector_cache_revision(\n            adapter.model_fingerprint(),\n            settings.score_threshold,\n            settings.nms_threshold,\n            settings.top_k,\n        );",
)
replace(
    "src/face_detection/yunet_production.rs",
    "    fn detector_version(&self) -> &'static str {\n        self.cache_revision\n    }\n\n    fn detect",
    "    fn detector_version(&self) -> &'static str {\n        MODEL_VERSION\n    }\n\n    fn cache_revision(&self) -> String {\n        self.cache_revision.clone()\n    }\n\n    fn detect",
)

# ---------------------------------------------------------------------------
# SFace production adapter: stable model version + content-aware revision.
# ---------------------------------------------------------------------------
replace(
    "src/face_sface_production.rs",
    "    cache_revision: &'static str,",
    "    cache_revision: String,",
)
replace(
    "src/face_sface_production.rs",
    "        let cache_revision = embedding_cache_revision(model_fingerprint);\n        let cache_revision = Box::leak(cache_revision.into_boxed_str());",
    "        let cache_revision = embedding_cache_revision(model_fingerprint);",
)
replace(
    "src/face_sface_production.rs",
    "    fn model_version(&self) -> &'static str {\n        self.cache_revision\n    }\n\n    fn input_size",
    "    fn model_version(&self) -> &'static str {\n        MODEL_VERSION\n    }\n\n    fn cache_revision(&self) -> String {\n        self.cache_revision.clone()\n    }\n\n    fn input_size",
)

# ---------------------------------------------------------------------------
# Face detection persistence: add detector_cache_revision without overloading
# detector_version. Keep legacy public helpers as compatibility wrappers.
# ---------------------------------------------------------------------------
replace(
    "src/face_store.rs",
    "    pub detector_version: String,\n    pub confidence: f32,",
    "    pub detector_version: String,\n    pub detector_cache_revision: String,\n    pub confidence: f32,",
)
replace(
    "src/face_store.rs",
    "    pub detector_version: String,\n    pub schema_version: i64,",
    "    pub detector_version: String,\n    pub detector_cache_revision: String,\n    pub schema_version: i64,",
)
replace(
    "src/face_store.rs",
    "            detector_version TEXT NOT NULL,\n            schema_version INTEGER NOT NULL,",
    "            detector_version TEXT NOT NULL,\n            detector_cache_revision TEXT NOT NULL DEFAULT '',\n            schema_version INTEGER NOT NULL,",
    2,
)
replace(
    "src/face_store.rs",
    "        \"#,\n    )?;\n    Ok(())\n}\n\n#[allow(clippy::too_many_arguments)]\npub fn detection_is_current(",
    "        \"#,\n    )?;\n    ensure_column(\n        conn,\n        \"face_detection_state\",\n        \"detector_cache_revision\",\n        \"TEXT NOT NULL DEFAULT ''\",\n    )?;\n    ensure_column(\n        conn,\n        \"faces\",\n        \"detector_cache_revision\",\n        \"TEXT NOT NULL DEFAULT ''\",\n    )?;\n    conn.execute(\n        \"UPDATE face_detection_state SET detector_cache_revision = detector_version WHERE detector_cache_revision = ''\",\n        [],\n    )?;\n    conn.execute(\n        \"UPDATE faces SET detector_cache_revision = detector_version WHERE detector_cache_revision = ''\",\n        [],\n    )?;\n    conn.execute_batch(\n        r#\"\n        CREATE INDEX IF NOT EXISTS idx_faces_detector_cache_revision\n            ON faces(detector_id, detector_version, detector_cache_revision, schema_version);\n        CREATE INDEX IF NOT EXISTS idx_face_detection_cache_revision\n            ON face_detection_state(detector_id, detector_version, detector_cache_revision, schema_version);\n        \"#,\n    )?;\n    Ok(())\n}\n\n#[allow(clippy::too_many_arguments)]\npub fn detection_is_current(",
)

old_detection = '''#[allow(clippy::too_many_arguments)]
pub fn detection_is_current(
    conn: &Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    detector_id: &str,
    detector_version: &str,
) -> Result<bool> {
    let current = conn.query_row(
        r#"
        SELECT 1
        FROM face_detection_state
        WHERE image_path = ?1
          AND detector_id = ?2
          AND detector_version = ?3
          AND schema_version = ?4
          AND source_size = ?5
          AND source_modified = ?6
        LIMIT 1
        "#,
        params![
            image_path.to_string_lossy().to_string(),
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            source_size as i64,
            source_modified,
        ],
        |_| Ok(()),
    );
    match current {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err.into()),
    }
}
'''
new_detection = '''#[allow(clippy::too_many_arguments)]
pub fn detection_is_current(
    conn: &Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    detector_id: &str,
    detector_version: &str,
) -> Result<bool> {
    detection_is_current_with_revision(
        conn,
        image_path,
        source_size,
        source_modified,
        detector_id,
        detector_version,
        detector_version,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn detection_is_current_with_revision(
    conn: &Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    detector_id: &str,
    detector_version: &str,
    detector_cache_revision: &str,
) -> Result<bool> {
    let current = conn.query_row(
        r#"
        SELECT 1
        FROM face_detection_state
        WHERE image_path = ?1
          AND detector_id = ?2
          AND detector_version = ?3
          AND detector_cache_revision = ?4
          AND schema_version = ?5
          AND source_size = ?6
          AND source_modified = ?7
        LIMIT 1
        "#,
        params![
            image_path.to_string_lossy().to_string(),
            detector_id,
            detector_version,
            detector_cache_revision,
            face_detection::SCHEMA_VERSION,
            source_size as i64,
            source_modified,
        ],
        |_| Ok(()),
    );
    match current {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(err) => Err(err.into()),
    }
}
'''
replace("src/face_store.rs", old_detection, new_detection)

old_paths = '''pub fn paths_needing_detection(
    conn: &Connection,
    detector_id: &str,
    detector_version: &str,
) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT images.path
        FROM images
        LEFT JOIN face_detection_state state ON state.image_path = images.path
        WHERE state.image_path IS NULL
           OR state.detector_id <> ?1
           OR state.detector_version <> ?2
           OR state.schema_version <> ?3
           OR state.source_size <> images.size
           OR state.source_modified <> images.modified
        ORDER BY images.path COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}
'''
new_paths = '''pub fn paths_needing_detection(
    conn: &Connection,
    detector_id: &str,
    detector_version: &str,
) -> Result<Vec<PathBuf>> {
    paths_needing_detection_with_revision(conn, detector_id, detector_version, detector_version)
}

pub fn paths_needing_detection_with_revision(
    conn: &Connection,
    detector_id: &str,
    detector_version: &str,
    detector_cache_revision: &str,
) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT images.path
        FROM images
        LEFT JOIN face_detection_state state ON state.image_path = images.path
        WHERE state.image_path IS NULL
           OR state.detector_id <> ?1
           OR state.detector_version <> ?2
           OR state.detector_cache_revision <> ?3
           OR state.schema_version <> ?4
           OR state.source_size <> images.size
           OR state.source_modified <> images.modified
        ORDER BY images.path COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            detector_id,
            detector_version,
            detector_cache_revision,
            face_detection::SCHEMA_VERSION
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}
'''
replace("src/face_store.rs", old_paths, new_paths)

old_replace_sig = '''#[allow(clippy::too_many_arguments)]
pub fn replace_detections(
    conn: &mut Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    exif_orientation: u32,
    oriented_width: u32,
    oriented_height: u32,
    detector_id: &str,
    detector_version: &str,
    detections: &[DetectedFace],
) -> Result<Vec<StoredFace>> {
'''
new_replace_sig = '''#[allow(clippy::too_many_arguments)]
pub fn replace_detections(
    conn: &mut Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    exif_orientation: u32,
    oriented_width: u32,
    oriented_height: u32,
    detector_id: &str,
    detector_version: &str,
    detections: &[DetectedFace],
) -> Result<Vec<StoredFace>> {
    replace_detections_with_revision(
        conn,
        image_path,
        source_size,
        source_modified,
        exif_orientation,
        oriented_width,
        oriented_height,
        detector_id,
        detector_version,
        detector_version,
        detections,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn replace_detections_with_revision(
    conn: &mut Connection,
    image_path: &Path,
    source_size: u64,
    source_modified: i64,
    exif_orientation: u32,
    oriented_width: u32,
    oriented_height: u32,
    detector_id: &str,
    detector_version: &str,
    detector_cache_revision: &str,
    detections: &[DetectedFace],
) -> Result<Vec<StoredFace>> {
'''
replace("src/face_store.rs", old_replace_sig, new_replace_sig)
replace(
    "src/face_store.rs",
    '    if detector_id.trim().is_empty() || detector_version.trim().is_empty() {\n        bail!("face detector id/version cannot be empty");\n    }',
    '    if detector_id.trim().is_empty()\n        || detector_version.trim().is_empty()\n        || detector_cache_revision.trim().is_empty()\n    {\n        bail!("face detector id/version/cache revision cannot be empty");\n    }',
)
replace(
    "src/face_store.rs",
    '''        INSERT INTO face_detection_state(
            image_path, detector_id, detector_version, schema_version,
            source_size, source_modified, exif_orientation,
            oriented_width, oriented_height, face_count
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
''',
    '''        INSERT INTO face_detection_state(
            image_path, detector_id, detector_version, detector_cache_revision, schema_version,
            source_size, source_modified, exif_orientation,
            oriented_width, oriented_height, face_count
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
''',
)
replace(
    "src/face_store.rs",
    '''            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            source_size as i64,
            source_modified,
            exif_orientation as i64,
            oriented_width as i64,
            oriented_height as i64,
            normalized.len() as i64,
''',
    '''            detector_id,
            detector_version,
            detector_cache_revision,
            face_detection::SCHEMA_VERSION,
            source_size as i64,
            source_modified,
            exif_orientation as i64,
            oriented_width as i64,
            oriented_height as i64,
            normalized.len() as i64,
''',
    1,
)
replace(
    "src/face_store.rs",
    '''            detector_id,
            detector_version,
            face_detection::SCHEMA_VERSION,
            ordinal,
''',
    '''            detector_id,
            detector_version,
            detector_cache_revision,
            face_detection::SCHEMA_VERSION,
            ordinal,
''',
)
replace(
    "src/face_store.rs",
    '''            INSERT INTO faces(
                face_id, image_path, face_ordinal, detector_id, detector_version,
                schema_version, confidence, bbox_x, bbox_y, bbox_width, bbox_height,
                landmarks, landmark_count, source_size, source_modified
            ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
            )
''',
    '''            INSERT INTO faces(
                face_id, image_path, face_ordinal, detector_id, detector_version,
                detector_cache_revision, schema_version, confidence, bbox_x, bbox_y, bbox_width,
                bbox_height, landmarks, landmark_count, source_size, source_modified
            ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )
''',
)
replace(
    "src/face_store.rs",
    '''                detector_id,
                detector_version,
                face_detection::SCHEMA_VERSION,
                face.confidence,
''',
    '''                detector_id,
                detector_version,
                detector_cache_revision,
                face_detection::SCHEMA_VERSION,
                face.confidence,
''',
)
replace(
    "src/face_store.rs",
    '''            detector_id: detector_id.to_owned(),
            detector_version: detector_version.to_owned(),
            confidence: face.confidence,
''',
    '''            detector_id: detector_id.to_owned(),
            detector_version: detector_version.to_owned(),
            detector_cache_revision: detector_cache_revision.to_owned(),
            confidence: face.confidence,
''',
)
replace(
    "src/face_store.rs",
    '''        SELECT image_path, detector_id, detector_version, schema_version,
               source_size, source_modified, exif_orientation,
               oriented_width, oriented_height, face_count
''',
    '''        SELECT image_path, detector_id, detector_version, detector_cache_revision, schema_version,
               source_size, source_modified, exif_orientation,
               oriented_width, oriented_height, face_count
''',
)
replace(
    "src/face_store.rs",
    '''                detector_id: row.get(1)?,
                detector_version: row.get(2)?,
                schema_version: row.get(3)?,
                source_size: row.get::<_, i64>(4)?.max(0) as u64,
                source_modified: row.get(5)?,
                exif_orientation: row.get::<_, i64>(6)?.clamp(1, 8) as u32,
                oriented_width: row.get::<_, i64>(7)?.max(0) as u32,
                oriented_height: row.get::<_, i64>(8)?.max(0) as u32,
                face_count: row.get::<_, i64>(9)?.max(0) as usize,
''',
    '''                detector_id: row.get(1)?,
                detector_version: row.get(2)?,
                detector_cache_revision: row.get(3)?,
                schema_version: row.get(4)?,
                source_size: row.get::<_, i64>(5)?.max(0) as u64,
                source_modified: row.get(6)?,
                exif_orientation: row.get::<_, i64>(7)?.clamp(1, 8) as u32,
                oriented_width: row.get::<_, i64>(8)?.max(0) as u32,
                oriented_height: row.get::<_, i64>(9)?.max(0) as u32,
                face_count: row.get::<_, i64>(10)?.max(0) as usize,
''',
)
replace(
    "src/face_store.rs",
    '''        SELECT face_id, image_path, face_ordinal, detector_id, detector_version,
               confidence, bbox_x, bbox_y, bbox_width, bbox_height,
               landmarks, landmark_count
''',
    '''        SELECT face_id, image_path, face_ordinal, detector_id, detector_version,
               detector_cache_revision, confidence, bbox_x, bbox_y, bbox_width, bbox_height,
               landmarks, landmark_count
''',
)
replace(
    "src/face_store.rs",
    '''        let blob: Option<Vec<u8>> = row.get(10)?;
        let count = row.get::<_, i64>(11)?.max(0) as usize;
''',
    '''        let blob: Option<Vec<u8>> = row.get(11)?;
        let count = row.get::<_, i64>(12)?.max(0) as usize;
''',
)
replace(
    "src/face_store.rs",
    '''            detector_id: row.get(3)?,
            detector_version: row.get(4)?,
            confidence: row.get(5)?,
            bbox: FaceBox {
                x: row.get(6)?,
                y: row.get(7)?,
                width: row.get(8)?,
                height: row.get(9)?,
''',
    '''            detector_id: row.get(3)?,
            detector_version: row.get(4)?,
            detector_cache_revision: row.get(5)?,
            confidence: row.get(6)?,
            bbox: FaceBox {
                x: row.get(7)?,
                y: row.get(8)?,
                width: row.get(9)?,
                height: row.get(10)?,
''',
)
replace(
    "src/face_store.rs",
    '''fn stable_face_id(
    image_path: &Path,
    detector_id: &str,
    detector_version: &str,
    schema_version: i64,
''',
    '''fn stable_face_id(
    image_path: &Path,
    detector_id: &str,
    detector_version: &str,
    detector_cache_revision: &str,
    schema_version: i64,
''',
)
replace(
    "src/face_store.rs",
    '''        detector_id.as_bytes(),
        detector_version.as_bytes(),
        &schema_version.to_le_bytes(),
''',
    '''        detector_id.as_bytes(),
        detector_version.as_bytes(),
        detector_cache_revision.as_bytes(),
        &schema_version.to_le_bytes(),
''',
)
replace(
    "src/face_store.rs",
    "fn compare_faces(left: &DetectedFace, right: &DetectedFace) -> std::cmp::Ordering {",
    '''fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.any(|row| row.is_ok_and(|name| name == column))
    };
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
            [],
        )?;
    }
    Ok(())
}

fn compare_faces(left: &DetectedFace, right: &DetectedFace) -> std::cmp::Ordering {''',
)
replace(
    "src/face_store.rs",
    "    #[test]\n    fn multiple_faces_are_sorted_and_keep_stable_ids_within_revision() {",
    '''    #[test]
    fn cache_revision_invalidates_without_changing_detector_version() {
        let mut conn = test_connection();
        insert_image(&conn, "people/revision.jpg");
        replace_detections_with_revision(
            &mut conn,
            Path::new("people/revision.jpg"),
            100,
            200,
            1,
            800,
            600,
            "test-detector",
            "1",
            "model-a",
            &[face(0.2, 0.95)],
        )
        .unwrap();
        assert!(detection_is_current_with_revision(
            &conn,
            Path::new("people/revision.jpg"),
            100,
            200,
            "test-detector",
            "1",
            "model-a",
        )
        .unwrap());
        assert!(!detection_is_current_with_revision(
            &conn,
            Path::new("people/revision.jpg"),
            100,
            200,
            "test-detector",
            "1",
            "model-b",
        )
        .unwrap());
        assert!(paths_needing_detection_with_revision(
            &conn,
            "test-detector",
            "1",
            "model-b",
        )
        .unwrap()
        .contains(&PathBuf::from("people/revision.jpg")));
    }

    #[test]
    fn multiple_faces_are_sorted_and_keep_stable_ids_within_revision() {''',
)

# ---------------------------------------------------------------------------
# Face embedding persistence: model cache revision and source detector cache
# revision are first-class columns. Compatibility wrappers preserve old tests/API.
# ---------------------------------------------------------------------------
replace(
    "src/face_embedding_store.rs",
    "    pub detector_version: String,\n    pub detection_schema_version: i64,",
    "    pub detector_version: String,\n    pub detector_cache_revision: String,\n    pub detection_schema_version: i64,",
    2,
)
replace(
    "src/face_embedding_store.rs",
    "    pub model_version: String,\n    pub schema_version: i64,",
    "    pub model_version: String,\n    pub model_cache_revision: String,\n    pub schema_version: i64,",
)
replace(
    "src/face_embedding_store.rs",
    "            model_version TEXT NOT NULL,\n            schema_version INTEGER NOT NULL,",
    "            model_version TEXT NOT NULL,\n            model_cache_revision TEXT NOT NULL DEFAULT '',\n            schema_version INTEGER NOT NULL,",
)
replace(
    "src/face_embedding_store.rs",
    "            detector_version TEXT NOT NULL,\n            detection_schema_version INTEGER NOT NULL,",
    "            detector_version TEXT NOT NULL,\n            detector_cache_revision TEXT NOT NULL DEFAULT '',\n            detection_schema_version INTEGER NOT NULL,",
)
replace(
    "src/face_embedding_store.rs",
    "        \"#,\n    )?;\n    Ok(())\n}\n\npub fn count_pending(",
    "        \"#,\n    )?;\n    ensure_column(\n        conn,\n        \"face_embeddings\",\n        \"model_cache_revision\",\n        \"TEXT NOT NULL DEFAULT ''\",\n    )?;\n    ensure_column(\n        conn,\n        \"face_embeddings\",\n        \"detector_cache_revision\",\n        \"TEXT NOT NULL DEFAULT ''\",\n    )?;\n    conn.execute(\n        \"UPDATE face_embeddings SET model_cache_revision = model_version WHERE model_cache_revision = ''\",\n        [],\n    )?;\n    conn.execute(\n        \"UPDATE face_embeddings SET detector_cache_revision = detector_version WHERE detector_cache_revision = ''\",\n        [],\n    )?;\n    conn.execute_batch(\n        r#\"\n        CREATE INDEX IF NOT EXISTS idx_face_embeddings_model_cache_revision\n            ON face_embeddings(model_id, model_version, model_cache_revision, schema_version, alignment_revision);\n        CREATE INDEX IF NOT EXISTS idx_face_embeddings_detection_cache_revision\n            ON face_embeddings(detector_id, detector_version, detector_cache_revision, detection_schema_version);\n        \"#,\n    )?;\n    Ok(())\n}\n\npub fn count_pending(",
)

old_count = '''pub fn count_pending(
    conn: &Connection,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<usize> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    let count = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE e.face_id IS NULL
           OR e.model_id <> ?1
           OR e.model_version <> ?2
           OR e.schema_version <> ?3
           OR e.alignment_revision <> ?4
           OR e.dimension <> ?5
           OR e.normalized <> 1
           OR e.detector_id <> f.detector_id
           OR e.detector_version <> f.detector_version
           OR e.detection_schema_version <> f.schema_version
           OR e.source_size <> f.source_size
           OR e.source_modified <> f.source_modified
        "#,
        params![
            model_id,
            model_version,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}
'''
new_count = '''pub fn count_pending(
    conn: &Connection,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<usize> {
    count_pending_with_revision(
        conn,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
    )
}

pub fn count_pending_with_revision(
    conn: &Connection,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<usize> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let count = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE e.face_id IS NULL
           OR e.model_id <> ?1
           OR e.model_version <> ?2
           OR e.model_cache_revision <> ?3
           OR e.schema_version <> ?4
           OR e.alignment_revision <> ?5
           OR e.dimension <> ?6
           OR e.normalized <> 1
           OR e.detector_id <> f.detector_id
           OR e.detector_version <> f.detector_version
           OR e.detector_cache_revision <> f.detector_cache_revision
           OR e.detection_schema_version <> f.schema_version
           OR e.source_size <> f.source_size
           OR e.source_modified <> f.source_modified
        "#,
        params![
            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}
'''
replace("src/face_embedding_store.rs", old_count, new_count)

old_candidate = '''pub fn candidate_batch(
    conn: &Connection,
    after_face_id: Option<&str>,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
    limit: usize,
) -> Result<Vec<EmbeddingCandidate>> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    let limit = limit.max(1);
    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id, f.image_path,
               f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height,
               f.landmarks, f.landmark_count,
               f.detector_id, f.detector_version, f.schema_version,
               f.source_size, f.source_modified
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE (?1 IS NULL OR f.face_id > ?1)
          AND (
               e.face_id IS NULL
            OR e.model_id <> ?2
            OR e.model_version <> ?3
            OR e.schema_version <> ?4
            OR e.alignment_revision <> ?5
            OR e.dimension <> ?6
            OR e.normalized <> 1
            OR e.detector_id <> f.detector_id
            OR e.detector_version <> f.detector_version
            OR e.detection_schema_version <> f.schema_version
            OR e.source_size <> f.source_size
            OR e.source_modified <> f.source_modified
          )
        ORDER BY f.face_id
        LIMIT ?7
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            after_face_id,
            model_id,
            model_version,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
            limit as i64,
        ],
        |row| {
            let landmark_blob: Option<Vec<u8>> = row.get(6)?;
            let landmark_count = row.get::<_, i64>(7)?.max(0) as usize;
            Ok(EmbeddingCandidate {
                face_id: row.get(0)?,
                image_path: PathBuf::from(row.get::<_, String>(1)?),
                bbox: FaceBox {
                    x: row.get(2)?,
                    y: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                },
                landmarks: landmark_blob
                    .as_deref()
                    .and_then(|bytes| decode_landmarks(bytes, landmark_count))
                    .unwrap_or_default(),
                detector_id: row.get(8)?,
                detector_version: row.get(9)?,
                detection_schema_version: row.get(10)?,
                source_size: row.get::<_, i64>(11)?.max(0) as u64,
                source_modified: row.get(12)?,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading pending face embedding candidates")
}
'''
new_candidate = '''pub fn candidate_batch(
    conn: &Connection,
    after_face_id: Option<&str>,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
    limit: usize,
) -> Result<Vec<EmbeddingCandidate>> {
    candidate_batch_with_revision(
        conn,
        after_face_id,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
        limit,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn candidate_batch_with_revision(
    conn: &Connection,
    after_face_id: Option<&str>,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
    limit: usize,
) -> Result<Vec<EmbeddingCandidate>> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let limit = limit.max(1);
    let mut stmt = conn.prepare(
        r#"
        SELECT f.face_id, f.image_path,
               f.bbox_x, f.bbox_y, f.bbox_width, f.bbox_height,
               f.landmarks, f.landmark_count,
               f.detector_id, f.detector_version, f.detector_cache_revision, f.schema_version,
               f.source_size, f.source_modified
        FROM faces f
        LEFT JOIN face_embeddings e ON e.face_id = f.face_id
        WHERE (?1 IS NULL OR f.face_id > ?1)
          AND (
               e.face_id IS NULL
            OR e.model_id <> ?2
            OR e.model_version <> ?3
            OR e.model_cache_revision <> ?4
            OR e.schema_version <> ?5
            OR e.alignment_revision <> ?6
            OR e.dimension <> ?7
            OR e.normalized <> 1
            OR e.detector_id <> f.detector_id
            OR e.detector_version <> f.detector_version
            OR e.detector_cache_revision <> f.detector_cache_revision
            OR e.detection_schema_version <> f.schema_version
            OR e.source_size <> f.source_size
            OR e.source_modified <> f.source_modified
          )
        ORDER BY f.face_id
        LIMIT ?8
        "#,
    )?;
    let rows = stmt.query_map(
        params![
            after_face_id,
            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            dimension as i64,
            limit as i64,
        ],
        |row| {
            let landmark_blob: Option<Vec<u8>> = row.get(6)?;
            let landmark_count = row.get::<_, i64>(7)?.max(0) as usize;
            Ok(EmbeddingCandidate {
                face_id: row.get(0)?,
                image_path: PathBuf::from(row.get::<_, String>(1)?),
                bbox: FaceBox {
                    x: row.get(2)?,
                    y: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                },
                landmarks: landmark_blob
                    .as_deref()
                    .and_then(|bytes| decode_landmarks(bytes, landmark_count))
                    .unwrap_or_default(),
                detector_id: row.get(8)?,
                detector_version: row.get(9)?,
                detector_cache_revision: row.get(10)?,
                detection_schema_version: row.get(11)?,
                source_size: row.get::<_, i64>(12)?.max(0) as u64,
                source_modified: row.get(13)?,
            })
        },
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("loading pending face embedding candidates")
}
'''
replace("src/face_embedding_store.rs", old_candidate, new_candidate)

old_current = '''pub fn embedding_is_current(
    conn: &Connection,
    face_id: &str,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<bool> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    let current = conn
        .query_row(
            r#"
            SELECT 1
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            WHERE e.face_id = ?1
              AND e.model_id = ?2
              AND e.model_version = ?3
              AND e.schema_version = ?4
              AND e.alignment_revision = ?5
              AND e.dimension = ?6
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
            LIMIT 1
            "#,
            params![
                face_id,
                model_id,
                model_version,
                face_embedding::SCHEMA_VERSION,
                alignment_revision,
                dimension as i64,
            ],
            |_| Ok(()),
        )
        .optional()?;
    Ok(current.is_some())
}
'''
new_current = '''pub fn embedding_is_current(
    conn: &Connection,
    face_id: &str,
    model_id: &str,
    model_version: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<bool> {
    embedding_is_current_with_revision(
        conn,
        face_id,
        model_id,
        model_version,
        model_version,
        dimension,
        alignment_revision,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn embedding_is_current_with_revision(
    conn: &Connection,
    face_id: &str,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    dimension: usize,
    alignment_revision: i64,
) -> Result<bool> {
    validate_model_revision(model_id, model_version, dimension, alignment_revision)?;
    validate_cache_revision(model_cache_revision)?;
    let current = conn
        .query_row(
            r#"
            SELECT 1
            FROM face_embeddings e
            JOIN faces f ON f.face_id = e.face_id
            WHERE e.face_id = ?1
              AND e.model_id = ?2
              AND e.model_version = ?3
              AND e.model_cache_revision = ?4
              AND e.schema_version = ?5
              AND e.alignment_revision = ?6
              AND e.dimension = ?7
              AND e.normalized = 1
              AND e.detector_id = f.detector_id
              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
              AND e.source_size = f.source_size
              AND e.source_modified = f.source_modified
            LIMIT 1
            "#,
            params![
                face_id,
                model_id,
                model_version,
                model_cache_revision,
                face_embedding::SCHEMA_VERSION,
                alignment_revision,
                dimension as i64,
            ],
            |_| Ok(()),
        )
        .optional()?;
    Ok(current.is_some())
}
'''
replace("src/face_embedding_store.rs", old_current, new_current)

old_replace_embedding_sig = '''pub fn replace_embedding(
    conn: &mut Connection,
    candidate: &EmbeddingCandidate,
    model_id: &str,
    model_version: &str,
    alignment_revision: i64,
    values: &[f32],
) -> Result<()> {
'''
new_replace_embedding_sig = '''pub fn replace_embedding(
    conn: &mut Connection,
    candidate: &EmbeddingCandidate,
    model_id: &str,
    model_version: &str,
    alignment_revision: i64,
    values: &[f32],
) -> Result<()> {
    replace_embedding_with_revision(
        conn,
        candidate,
        model_id,
        model_version,
        model_version,
        alignment_revision,
        values,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn replace_embedding_with_revision(
    conn: &mut Connection,
    candidate: &EmbeddingCandidate,
    model_id: &str,
    model_version: &str,
    model_cache_revision: &str,
    alignment_revision: i64,
    values: &[f32],
) -> Result<()> {
'''
replace("src/face_embedding_store.rs", old_replace_embedding_sig, new_replace_embedding_sig)
replace(
    "src/face_embedding_store.rs",
    "    validate_model_revision(model_id, model_version, values.len(), alignment_revision)?;\n    if values.iter()",
    "    validate_model_revision(model_id, model_version, values.len(), alignment_revision)?;\n    validate_cache_revision(model_cache_revision)?;\n    if values.iter()",
)
replace(
    "src/face_embedding_store.rs",
    '''              AND detector_version = ?4
              AND schema_version = ?5
              AND source_size = ?6
              AND source_modified = ?7
''',
    '''              AND detector_version = ?4
              AND detector_cache_revision = ?5
              AND schema_version = ?6
              AND source_size = ?7
              AND source_modified = ?8
''',
)
replace(
    "src/face_embedding_store.rs",
    '''            candidate.detector_id,
            candidate.detector_version,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
''',
    '''            candidate.detector_id,
            candidate.detector_version,
            candidate.detector_cache_revision,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
''',
    1,
)
replace(
    "src/face_embedding_store.rs",
    '''        INSERT INTO face_embeddings(
            face_id, model_id, model_version, schema_version, alignment_revision,
            dimension, normalized, embedding,
            detector_id, detector_version, detection_schema_version,
            source_size, source_modified, completed_at
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, ?9, ?10, ?11, ?12, unixepoch())
''',
    '''        INSERT INTO face_embeddings(
            face_id, model_id, model_version, model_cache_revision, schema_version, alignment_revision,
            dimension, normalized, embedding,
            detector_id, detector_version, detector_cache_revision, detection_schema_version,
            source_size, source_modified, completed_at
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14, unixepoch())
''',
)
replace(
    "src/face_embedding_store.rs",
    '''            model_version = excluded.model_version,
            schema_version = excluded.schema_version,
''',
    '''            model_version = excluded.model_version,
            model_cache_revision = excluded.model_cache_revision,
            schema_version = excluded.schema_version,
''',
)
replace(
    "src/face_embedding_store.rs",
    '''            detector_version = excluded.detector_version,
            detection_schema_version = excluded.detection_schema_version,
''',
    '''            detector_version = excluded.detector_version,
            detector_cache_revision = excluded.detector_cache_revision,
            detection_schema_version = excluded.detection_schema_version,
''',
)
replace(
    "src/face_embedding_store.rs",
    '''            model_id,
            model_version,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            values.len() as i64,
            blob,
            candidate.detector_id,
            candidate.detector_version,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
''',
    '''            model_id,
            model_version,
            model_cache_revision,
            face_embedding::SCHEMA_VERSION,
            alignment_revision,
            values.len() as i64,
            blob,
            candidate.detector_id,
            candidate.detector_version,
            candidate.detector_cache_revision,
            candidate.detection_schema_version,
            candidate.source_size as i64,
            candidate.source_modified,
''',
)
replace(
    "src/face_embedding_store.rs",
    '''            SELECT face_id, model_id, model_version, schema_version, alignment_revision,
                   dimension, normalized, embedding,
                   detector_id, detector_version, detection_schema_version,
                   source_size, source_modified
''',
    '''            SELECT face_id, model_id, model_version, model_cache_revision, schema_version, alignment_revision,
                   dimension, normalized, embedding,
                   detector_id, detector_version, detector_cache_revision, detection_schema_version,
                   source_size, source_modified
''',
)
replace(
    "src/face_embedding_store.rs",
    '''                let dimension = row.get::<_, i64>(5)?.max(0) as usize;
                let blob: Vec<u8> = row.get(7)?;
''',
    '''                let dimension = row.get::<_, i64>(6)?.max(0) as usize;
                let blob: Vec<u8> = row.get(8)?;
''',
)
replace(
    "src/face_embedding_store.rs",
    '''                    model_id: row.get(1)?,
                    model_version: row.get(2)?,
                    schema_version: row.get(3)?,
                    alignment_revision: row.get(4)?,
                    dimension,
                    normalized: row.get::<_, i64>(6)? != 0,
                    values,
                    detector_id: row.get(8)?,
                    detector_version: row.get(9)?,
                    detection_schema_version: row.get(10)?,
                    source_size: row.get::<_, i64>(11)?.max(0) as u64,
                    source_modified: row.get(12)?,
''',
    '''                    model_id: row.get(1)?,
                    model_version: row.get(2)?,
                    model_cache_revision: row.get(3)?,
                    schema_version: row.get(4)?,
                    alignment_revision: row.get(5)?,
                    dimension,
                    normalized: row.get::<_, i64>(7)? != 0,
                    values,
                    detector_id: row.get(9)?,
                    detector_version: row.get(10)?,
                    detector_cache_revision: row.get(11)?,
                    detection_schema_version: row.get(12)?,
                    source_size: row.get::<_, i64>(13)?.max(0) as u64,
                    source_modified: row.get(14)?,
''',
)
replace(
    "src/face_embedding_store.rs",
    "fn encode_embedding(values: &[f32]) -> Vec<u8> {",
    '''fn validate_cache_revision(cache_revision: &str) -> Result<()> {
    if cache_revision.trim().is_empty() {
        bail!("face embedding cache revision cannot be empty");
    }
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let exists = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.any(|row| row.is_ok_and(|name| name == column))
    };
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
            [],
        )?;
    }
    Ok(())
}

fn encode_embedding(values: &[f32]) -> Vec<u8> {''',
)
replace(
    "src/face_embedding_store.rs",
    "    #[test]\n    fn replacing_detection_cascades_embedding() {",
    '''    #[test]
    fn cache_revision_backfills_without_changing_model_version() {
        let (mut conn, candidate) = connection_with_face();
        let values = vec![0.6, 0.8];
        replace_embedding_with_revision(
            &mut conn,
            &candidate,
            "fake-embedder",
            "1",
            "weights-a",
            1,
            &values,
        )
        .unwrap();
        assert!(embedding_is_current_with_revision(
            &conn,
            &candidate.face_id,
            "fake-embedder",
            "1",
            "weights-a",
            2,
            1,
        )
        .unwrap());
        assert!(!embedding_is_current_with_revision(
            &conn,
            &candidate.face_id,
            "fake-embedder",
            "1",
            "weights-b",
            2,
            1,
        )
        .unwrap());
        assert_eq!(
            count_pending_with_revision(&conn, "fake-embedder", "1", "weights-b", 2, 1)
                .unwrap(),
            1
        );
    }

    #[test]
    fn replacing_detection_cascades_embedding() {''',
)

# ---------------------------------------------------------------------------
# Pipelines use stable version and independent cache revision.
# ---------------------------------------------------------------------------
replace(
    "src/face_pipeline.rs",
    "    let detector_version = detector.detector_version();\n    let session_conn",
    "    let detector_version = detector.detector_version();\n    let detector_cache_revision = detector.cache_revision();\n    let session_conn",
)
replace(
    "src/face_pipeline.rs",
    "                if face_store::detection_is_current(\n                    &conn,\n                    &relative,\n                    size,\n                    modified,\n                    detector_id,\n                    detector_version,\n                )? {",
    "                if face_store::detection_is_current_with_revision(\n                    &conn,\n                    &relative,\n                    size,\n                    modified,\n                    detector_id,\n                    detector_version,\n                    &detector_cache_revision,\n                )? {",
)
replace(
    "src/face_pipeline.rs",
    "                    let stored = face_store::replace_detections(\n                        &mut conn,",
    "                    let stored = face_store::replace_detections_with_revision(\n                        &mut conn,",
)
replace(
    "src/face_pipeline.rs",
    "                        detector_id,\n                        detector_version,\n                        &detections,",
    "                        detector_id,\n                        detector_version,\n                        &detector_cache_revision,\n                        &detections,",
    1,
)

replace(
    "src/face_embedding_pipeline.rs",
    "    let model_version = embedder.model_version();\n    let dimension",
    "    let model_version = embedder.model_version();\n    let model_cache_revision = embedder.cache_revision();\n    let dimension",
)
replace(
    "src/face_embedding_pipeline.rs",
    "        let pending = face_embedding_store::count_pending(\n            &conn,\n            model_id,\n            model_version,\n            dimension,\n            alignment_revision,\n        )?;",
    "        let pending = face_embedding_store::count_pending_with_revision(\n            &conn,\n            model_id,\n            model_version,\n            &model_cache_revision,\n            dimension,\n            alignment_revision,\n        )?;",
)
replace(
    "src/face_embedding_pipeline.rs",
    "            let batch = face_embedding_store::candidate_batch(\n                &conn,\n                cursor.as_deref(),\n                model_id,\n                model_version,\n                dimension,\n                alignment_revision,\n                options.batch_size,\n            )?;",
    "            let batch = face_embedding_store::candidate_batch_with_revision(\n                &conn,\n                cursor.as_deref(),\n                model_id,\n                model_version,\n                &model_cache_revision,\n                dimension,\n                alignment_revision,\n                options.batch_size,\n            )?;",
)
replace(
    "src/face_embedding_pipeline.rs",
    "                    face_embedding_store::replace_embedding(\n                        &mut conn,\n                        &candidate,\n                        model_id,\n                        model_version,\n                        alignment_revision,\n                        &normalized,\n                    )?;",
    "                    face_embedding_store::replace_embedding_with_revision(\n                        &mut conn,\n                        &candidate,\n                        model_id,\n                        model_version,\n                        &model_cache_revision,\n                        alignment_revision,\n                        &normalized,\n                    )?;",
)

# ---------------------------------------------------------------------------
# Similarity search never mixes embeddings from different external model files.
# ---------------------------------------------------------------------------
replace(
    "src/face_similarity.rs",
    "    pub model_version: String,\n    pub schema_version: i64,",
    "    pub model_version: String,\n    pub model_cache_revision: String,\n    pub schema_version: i64,",
)
replace(
    "src/face_similarity.rs",
    '''            SELECT f.image_path,
                   e.model_id, e.model_version, e.schema_version,
                   e.alignment_revision, e.dimension, e.embedding
''',
    '''            SELECT f.image_path,
                   e.model_id, e.model_version, e.model_cache_revision, e.schema_version,
                   e.alignment_revision, e.dimension, e.embedding
''',
)
replace(
    "src/face_similarity.rs",
    '''              AND e.detector_version = f.detector_version
              AND e.detection_schema_version = f.schema_version
''',
    '''              AND e.detector_version = f.detector_version
              AND e.detector_cache_revision = f.detector_cache_revision
              AND e.detection_schema_version = f.schema_version
''',
    2,
)
replace(
    "src/face_similarity.rs",
    '''              AND s.detector_version = f.detector_version
              AND s.schema_version = f.schema_version
''',
    '''              AND s.detector_version = f.detector_version
              AND s.detector_cache_revision = f.detector_cache_revision
              AND s.schema_version = f.schema_version
''',
    2,
)
replace(
    "src/face_similarity.rs",
    '''                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
''',
    '''                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
''',
)
replace(
    "src/face_similarity.rs",
    '''    let (image_path, model_id, model_version, schema_version, alignment_revision, dimension, blob) =
        row.context("query face has no current compatible embedding")?;
''',
    '''    let (
        image_path,
        model_id,
        model_version,
        model_cache_revision,
        schema_version,
        alignment_revision,
        dimension,
        blob,
    ) = row.context("query face has no current compatible embedding")?;
''',
)
replace(
    "src/face_similarity.rs",
    '''            model_id,
            model_version,
            schema_version,
''',
    '''            model_id,
            model_version,
            model_cache_revision,
            schema_version,
''',
    1,
)
replace(
    "src/face_similarity.rs",
    '''          AND e.model_id = ?2
          AND e.model_version = ?3
          AND e.schema_version = ?4
          AND e.alignment_revision = ?5
          AND e.dimension = ?6
''',
    '''          AND e.model_id = ?2
          AND e.model_version = ?3
          AND e.model_cache_revision = ?4
          AND e.schema_version = ?5
          AND e.alignment_revision = ?6
          AND e.dimension = ?7
''',
)
replace(
    "src/face_similarity.rs",
    "        LIMIT ?7\n",
    "        LIMIT ?8\n",
)
replace(
    "src/face_similarity.rs",
    '''            revision.model_id,
            revision.model_version,
            revision.schema_version,
            revision.alignment_revision,
            revision.dimension as i64,
            limit.max(1) as i64,
''',
    '''            revision.model_id,
            revision.model_version,
            revision.model_cache_revision,
            revision.schema_version,
            revision.alignment_revision,
            revision.dimension as i64,
            limit.max(1) as i64,
''',
)
replace(
    "src/face_similarity.rs",
    '''        || query.revision.model_version.trim().is_empty()
        || query.revision.schema_version != face_embedding::SCHEMA_VERSION
''',
    '''        || query.revision.model_version.trim().is_empty()
        || query.revision.model_cache_revision.trim().is_empty()
        || query.revision.schema_version != face_embedding::SCHEMA_VERSION
''',
)
replace(
    "src/face_similarity.rs",
    "    #[test]\n    fn bounded_top_k_is_deterministic_for_equal_scores() {",
    '''    #[test]
    fn model_cache_revision_mismatch_is_excluded() {
        let query_root = setup_root("cache-query", "lib-a");
        let candidate_root = setup_root("cache-candidate", "lib-b");
        let query_face =
            add_image_faces(&query_root, "query.jpg", &[vec![1.0, 0.0]], "embedder").remove(0);
        let candidate_face = add_image_faces(
            &candidate_root,
            "candidate.jpg",
            &[vec![1.0, 0.0]],
            "embedder",
        )
        .remove(0);
        let conn = db::open(&portable::index_db_path(&candidate_root)).unwrap();
        conn.execute(
            "UPDATE face_embeddings SET model_cache_revision = 'different-weights' WHERE face_id = ?1",
            params![candidate_face],
        )
        .unwrap();

        let query = load_query(&query_root, &query_face).unwrap();
        let report = search_available_roots(
            &[candidate_root.clone()],
            &query,
            FaceSimilarityOptions::default(),
        )
        .unwrap();
        assert!(report.matches.is_empty());

        let _ = std::fs::remove_dir_all(query_root);
        let _ = std::fs::remove_dir_all(candidate_root);
    }

    #[test]
    fn bounded_top_k_is_deterministic_for_equal_scores() {''',
)

print("face cache revision patch applied")
