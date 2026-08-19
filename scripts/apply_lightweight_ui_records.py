from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# db.rs: introduce an actual lightweight UI type and a query that never selects
# visual feature blobs.
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")

record_marker = '''#[derive(Clone, Debug)]
pub struct ImageRecord {
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub description: String,
    pub keywords: String,
    pub dominant: [u8; 3],
    pub visual_hash: Option<u64>,
    pub color_histogram: Option<Vec<f32>>,
    pub embedding: Option<Vec<f32>>,
    pub score: Option<f32>,
}
'''
summary = record_marker + '''
#[derive(Clone, Debug)]
pub struct ImageSummary {
    pub path: PathBuf,
    pub root: PathBuf,
    pub file_name: String,
    pub extension: String,
    pub size: u64,
    pub modified: i64,
    pub width: u32,
    pub height: u32,
    pub description: String,
    pub keywords: String,
    pub dominant: [u8; 3],
    pub score: Option<f32>,
}

impl From<ImageRecord> for ImageSummary {
    fn from(record: ImageRecord) -> Self {
        Self {
            path: record.path,
            root: record.root,
            file_name: record.file_name,
            extension: record.extension,
            size: record.size,
            modified: record.modified,
            width: record.width,
            height: record.height,
            description: record.description,
            keywords: record.keywords,
            dominant: record.dominant,
            score: record.score,
        }
    }
}
'''
text = replace_once(text, record_marker, summary, "ImageSummary type")

load_pos = text.index("pub fn load_images(db_path: &Path) -> Result<Vec<ImageRecord>> {")
summary_query = '''pub fn load_image_summaries(db_path: &Path) -> Result<Vec<ImageSummary>> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT path, root, file_name, extension, size, modified, width, height,
               description, keywords, dominant_r, dominant_g, dominant_b
        FROM images
        ORDER BY file_name COLLATE NOCASE
        "#,
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(ImageSummary {
            path: PathBuf::from(row.get::<_, String>(0)?),
            root: PathBuf::from(row.get::<_, String>(1)?),
            file_name: row.get(2)?,
            extension: row.get(3)?,
            size: row.get::<_, i64>(4)?.max(0) as u64,
            modified: row.get(5)?,
            width: row.get::<_, i64>(6)?.max(0) as u32,
            height: row.get::<_, i64>(7)?.max(0) as u32,
            description: row.get(8)?,
            keywords: row.get(9)?,
            dominant: [
                row.get::<_, i64>(10)?.clamp(0, 255) as u8,
                row.get::<_, i64>(11)?.clamp(0, 255) as u8,
                row.get::<_, i64>(12)?.clamp(0, 255) as u8,
            ],
            score: None,
        })
    })?;

    Ok(rows.filter_map(|row| row.ok()).collect())
}

'''
text = text[:load_pos] + summary_query + text[load_pos:]

# Add a regression test proving UI summaries never load CLIP/descriptor blobs.
test_marker = '''    #[test]
    fn load_file_states_returns_all_persisted_rows() {
'''
test_insert = '''    #[test]
    fn lightweight_summaries_match_metadata_without_feature_blobs() {
        let db_path = temp_db_path("lightweight-summary");
        let root = std::env::temp_dir().join("windows-image-search-summary-root");
        let image = root.join("sample.jpg");

        {
            let conn = open(&db_path).unwrap();
            upsert_image(
                &conn,
                &image,
                &root,
                "sample.jpg",
                "jpg",
                1234,
                5678,
                320,
                240,
                "description",
                "keyword",
                [12, 34, 56],
                0x1234,
                &[0.2, 0.8],
            )
            .unwrap();
            set_embedding(&conn, &image, &[0.1, 0.2, 0.3, 0.4]).unwrap();
        }

        let full = load_images(&db_path).unwrap();
        let summaries = load_image_summaries(&db_path).unwrap();
        assert_eq!(full.len(), 1);
        assert_eq!(summaries.len(), 1);
        assert!(full[0].embedding.is_some());
        assert!(full[0].color_histogram.is_some());
        assert_eq!(summaries[0].path, full[0].path);
        assert_eq!(summaries[0].file_name, full[0].file_name);
        assert_eq!(summaries[0].description, full[0].description);
        assert_eq!(summaries[0].dominant, full[0].dominant);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn load_file_states_returns_all_persisted_rows() {
'''
text = replace_once(text, test_marker, test_insert, "summary regression test")
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# indexer.rs: worker messages carry lightweight summaries, and similarity search
# consumes heavy records internally then drops all feature blobs before UI handoff.
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::db::{self, ImageRecord};\n",
    "use crate::db::{self, ImageRecord, ImageSummary};\n",
    "indexer summary import",
)
text = replace_once(
    text,
    "    fn to_record(&self) -> ImageRecord {\n        ImageRecord {\n",
    "    fn to_summary(&self) -> ImageSummary {\n        ImageSummary {\n",
    "PreparedImage summary conversion",
)
# Remove search-only fields from the PreparedImage -> UI conversion.
text = replace_once(
    text,
    '''            dominant: self.dominant,
            visual_hash: Some(self.visual_hash),
            color_histogram: Some(self.color_histogram.clone()),
            embedding: None,
            score: None,
''',
    '''            dominant: self.dominant,
            score: None,
''',
    "PreparedImage lightweight fields",
)
text = replace_once(
    text,
    "    IndexedBatch(Vec<ImageRecord>),\n    SimilarityResults(Vec<ImageRecord>),\n",
    "    IndexedBatch(Vec<ImageSummary>),\n    SimilarityResults(Vec<ImageSummary>),\n",
    "lightweight worker message types",
)
text = replace_once(
    text,
    "        let live_records = prepared.iter().map(PreparedImage::to_record).collect();\n",
    "        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();\n",
    "live batch lightweight summaries",
)
text = replace_once(
    text,
    ") -> Result<Vec<ImageRecord>> {\n    let indexing_settings = indexing_settings.sanitized();\n",
    ") -> Result<Vec<ImageSummary>> {\n    let indexing_settings = indexing_settings.sanitized();\n",
    "similarity returns summaries",
)
text = replace_once(
    text,
    '''    records.sort_by(|a, b| {
        let a_exact = normalized_path_key(&a.path) == query_key;
        let b_exact = normalized_path_key(&b.path) == query_key;
        b_exact.cmp(&a_exact).then_with(|| {
            b.score
                .unwrap_or(f32::NEG_INFINITY)
                .total_cmp(&a.score.unwrap_or(f32::NEG_INFINITY))
        })
    });
    Ok(records)
}
''',
    '''    records.sort_by(|a, b| {
        let a_exact = normalized_path_key(&a.path) == query_key;
        let b_exact = normalized_path_key(&b.path) == query_key;
        b_exact.cmp(&a_exact).then_with(|| {
            b.score
                .unwrap_or(f32::NEG_INFINITY)
                .total_cmp(&a.score.unwrap_or(f32::NEG_INFINITY))
        })
    });
    Ok(records.into_iter().map(ImageSummary::from).collect())
}
''',
    "drop heavy similarity features before UI",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/mod.rs: browse and result state is lightweight end-to-end.
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::db::{self, ImageRecord};\n",
    "use crate::db::{self, ImageSummary};\n",
    "UI summary import",
)
text = text.replace("Vec<ImageRecord>", "Vec<ImageSummary>")
text = text.replace("&[ImageRecord]", "&[ImageSummary]")
text = text.replace("db::load_images(&db_path)", "db::load_image_summaries(&db_path)")
text = text.replace("db::load_images(&self.db_path)", "db::load_image_summaries(&self.db_path)")
if "ImageRecord" in text:
    raise SystemExit("ImageRecord remains in ui/mod.rs")
path.write_text(text, encoding="utf-8")

print("Lightweight UI records patch applied")
