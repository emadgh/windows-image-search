use crate::{db, oversized_preview, portable, settings};
use anyhow::{bail, Context, Result};
use image::GenericImageView;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const PORTABLE_FORMAT_MARKER: &str = "windows-image-search-portable";
const DEFAULT_BATCH_SIZE: usize = 512;
const MAX_BATCH_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyMode {
    Quick,
    DeepFingerprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifyOptions {
    pub mode: VerifyMode,
    pub batch_size: usize,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            mode: VerifyMode::Quick,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl VerifyOptions {
    fn sanitized(self) -> Self {
        Self {
            mode: self.mode,
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyProblemKind {
    UnsafeRelativePath,
    MissingSource,
    SourceSizeChanged,
    SourceModifiedChanged,
    InvalidColorHistogram,
    InvalidMaterialTexture,
    InvalidEmbedding,
    MissingFingerprint,
    FingerprintMismatch,
    SourceDecodeFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyProblem {
    pub path: PathBuf,
    pub kind: VerifyProblemKind,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub library_id: String,
    pub schema_version: i64,
    pub records_checked: usize,
    pub deep_fingerprints_checked: usize,
    pub problems: Vec<VerifyProblem>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn render_text(&self, root: &Path, mode: VerifyMode) -> String {
        let mut text = String::new();
        text.push_str("Windows Image Search Portable Index Verification\n");
        text.push_str(&format!("root={}\n", root.display()));
        text.push_str(&format!("library_id={}\n", self.library_id));
        text.push_str(&format!("schema_version={}\n", self.schema_version));
        text.push_str(&format!(
            "mode={}\n",
            match mode {
                VerifyMode::Quick => "quick",
                VerifyMode::DeepFingerprint => "deep-fingerprint",
            }
        ));
        text.push_str(&format!("records_checked={}\n", self.records_checked));
        text.push_str(&format!(
            "deep_fingerprints_checked={}\n",
            self.deep_fingerprints_checked
        ));
        text.push_str(&format!("problems={}\n", self.problems.len()));
        text.push_str(&format!("clean={}\n", self.is_clean()));
        for problem in &self.problems {
            text.push_str(&format!(
                "problem\t{:?}\t{}\t{}\n",
                problem.kind,
                problem.path.display(),
                problem.detail.replace(['\t', '\r', '\n'], " ")
            ));
        }
        text
    }
}

#[derive(Clone, Debug)]
struct PortableMetadata {
    library_id: String,
    schema_version: i64,
}

pub fn preflight_existing_index(root: &Path) -> Result<()> {
    let path = portable::index_db_path(root);
    if !path.is_file() {
        return Ok(());
    }
    let conn = open_read_only(&path)?;
    let _ = validate_metadata(&conn, &path)?;
    Ok(())
}

pub fn verify_root<F>(root: &Path, options: VerifyOptions, mut progress: F) -> Result<VerifyReport>
where
    F: FnMut(usize),
{
    if !root.is_dir() {
        bail!("portable root is unavailable: {}", root.display());
    }
    let db_path = portable::index_db_path(root);
    if !db_path.is_file() {
        bail!("portable index does not exist: {}", db_path.display());
    }
    let options = options.sanitized();
    let conn = open_read_only(&db_path)?;
    let metadata = validate_metadata(&conn, &db_path)?;
    validate_images_table(&conn, &db_path)?;

    let mut report = VerifyReport {
        library_id: metadata.library_id,
        schema_version: metadata.schema_version,
        ..VerifyReport::default()
    };
    let mut cursor: Option<String> = None;

    loop {
        let mut stmt = conn.prepare(
            r#"
            SELECT path, size, modified,
                   length(color_histogram), COALESCE(color_histogram_dim, 0),
                   length(material_texture), COALESCE(material_texture_dim, 0),
                   length(embedding), COALESCE(embedding_dim, 0),
                   content_fingerprint
            FROM images
            WHERE (?1 IS NULL OR path > ?1)
            ORDER BY path
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(
            params![cursor.as_deref(), options.batch_size as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                ))
            },
        )?;
        let batch: Vec<_> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            break;
        }

        for (
            path_text,
            stored_size,
            stored_modified,
            histogram_bytes,
            histogram_dim,
            texture_bytes,
            texture_dim,
            embedding_bytes,
            embedding_dim,
            fingerprint,
        ) in batch
        {
            cursor = Some(path_text.clone());
            report.records_checked += 1;
            let relative = PathBuf::from(&path_text);
            let absolute = match portable::absolute_source_path(root, &relative) {
                Ok(path) => path,
                Err(err) => {
                    push_problem(
                        &mut report,
                        relative,
                        VerifyProblemKind::UnsafeRelativePath,
                        format!("{err:#}"),
                    );
                    progress(report.records_checked);
                    continue;
                }
            };

            check_blob_dimension(
                &mut report,
                &relative,
                "color_histogram",
                histogram_bytes,
                histogram_dim,
                VerifyProblemKind::InvalidColorHistogram,
            );
            check_blob_dimension(
                &mut report,
                &relative,
                "material_texture",
                texture_bytes,
                texture_dim,
                VerifyProblemKind::InvalidMaterialTexture,
            );
            check_blob_dimension(
                &mut report,
                &relative,
                "embedding",
                embedding_bytes,
                embedding_dim,
                VerifyProblemKind::InvalidEmbedding,
            );

            let source_meta = match std::fs::metadata(&absolute) {
                Ok(meta) if meta.is_file() => meta,
                Ok(_) => {
                    push_problem(
                        &mut report,
                        relative.clone(),
                        VerifyProblemKind::MissingSource,
                        "source path is not a regular file".to_owned(),
                    );
                    progress(report.records_checked);
                    continue;
                }
                Err(err) => {
                    push_problem(
                        &mut report,
                        relative.clone(),
                        VerifyProblemKind::MissingSource,
                        err.to_string(),
                    );
                    progress(report.records_checked);
                    continue;
                }
            };

            if source_meta.len() != stored_size.max(0) as u64 {
                push_problem(
                    &mut report,
                    relative.clone(),
                    VerifyProblemKind::SourceSizeChanged,
                    format!("stored={stored_size} actual={}", source_meta.len()),
                );
            }
            let actual_modified = modified_seconds(&source_meta);
            if actual_modified != stored_modified {
                push_problem(
                    &mut report,
                    relative.clone(),
                    VerifyProblemKind::SourceModifiedChanged,
                    format!("stored={stored_modified} actual={actual_modified}"),
                );
            }

            if options.mode == VerifyMode::DeepFingerprint {
                match fingerprint {
                    None => push_problem(
                        &mut report,
                        relative.clone(),
                        VerifyProblemKind::MissingFingerprint,
                        "no stored content fingerprint".to_owned(),
                    ),
                    Some(stored) => {
                        let provenance = db::descriptor_provenance(&conn, &relative)?;
                        let oversized =
                            stored_size.max(0) as u64 > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES;
                        if oversized
                            && !provenance.is_some_and(|value| {
                                value.source == db::DescriptorSource::OversizedPreview
                            })
                        {
                            push_problem(
                                &mut report,
                                relative.clone(),
                                VerifyProblemKind::SourceDecodeFailed,
                                "oversized source has unsafe direct provenance; refusing full decode"
                                    .to_owned(),
                            );
                            progress(report.records_checked);
                            continue;
                        }
                        let fingerprint_result = if oversized {
                            oversized_preview::load_or_build(
                                root,
                                &absolute,
                                stored_size.max(0) as u64,
                                stored_modified,
                            )
                            .map(|asset| decoded_image_fingerprint(&asset.image))
                        } else {
                            decoded_content_fingerprint(&absolute)
                        };
                        match fingerprint_result {
                            Ok(actual) => {
                                report.deep_fingerprints_checked += 1;
                                if actual as i64 != stored {
                                    push_problem(
                                        &mut report,
                                        relative.clone(),
                                        VerifyProblemKind::FingerprintMismatch,
                                        format!(
                                            "stored={:016x} actual={actual:016x}",
                                            stored as u64
                                        ),
                                    );
                                }
                            }
                            Err(err) => push_problem(
                                &mut report,
                                relative.clone(),
                                VerifyProblemKind::SourceDecodeFailed,
                                format!("{err:#}"),
                            ),
                        }
                    }
                }
            }
            progress(report.records_checked);
        }
    }
    Ok(report)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable index read-only {}", path.display()))
}

fn validate_metadata(conn: &Connection, path: &Path) -> Result<PortableMetadata> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='portable_meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        bail!(
            "existing portable index has no portable_meta table; refusing mutation: {}",
            path.display()
        );
    }
    let schema_text = meta_value(conn, "schema_version")?
        .context("portable index has no schema_version metadata; refusing mutation")?;
    let schema_version = schema_text
        .parse::<i64>()
        .with_context(|| format!("invalid portable schema_version {schema_text:?}"))?;
    if schema_version <= 0 {
        bail!("portable schema_version must be positive, got {schema_version}");
    }
    if schema_version > portable::PORTABLE_SCHEMA_VERSION {
        bail!(
            "portable index schema v{schema_version} is newer than supported v{}; refusing mutation",
            portable::PORTABLE_SCHEMA_VERSION
        );
    }
    if let Some(marker) = meta_value(conn, "format")? {
        if marker != PORTABLE_FORMAT_MARKER {
            bail!("unknown portable index format marker {marker:?}; refusing mutation");
        }
    }
    let library_id = meta_value(conn, "library_id")?
        .context("portable index has no library_id metadata; refusing mutation")?;
    if library_id.trim().is_empty() {
        bail!("portable index library_id is empty; refusing mutation");
    }
    Ok(PortableMetadata {
        library_id,
        schema_version,
    })
}

fn validate_images_table(conn: &Connection, path: &Path) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='images')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        bail!("portable index has no images table: {}", path.display());
    }
    Ok(())
}

fn meta_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM portable_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn check_blob_dimension(
    report: &mut VerifyReport,
    path: &Path,
    label: &str,
    bytes: Option<i64>,
    dimension: i64,
    kind: VerifyProblemKind,
) {
    let valid = match bytes {
        None => dimension == 0,
        Some(length) => dimension > 0 && length == dimension.saturating_mul(4),
    };
    if !valid {
        push_problem(
            report,
            path.to_path_buf(),
            kind,
            format!("{label}: blob_bytes={bytes:?} dimension={dimension}"),
        );
    }
}

fn push_problem(report: &mut VerifyReport, path: PathBuf, kind: VerifyProblemKind, detail: String) {
    report.problems.push(VerifyProblem { path, kind, detail });
}

fn modified_seconds(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

fn decoded_content_fingerprint(path: &Path) -> Result<u64> {
    let image = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    Ok(decoded_image_fingerprint(&image))
}

fn decoded_image_fingerprint(image: &image::DynamicImage) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in image
        .width()
        .to_le_bytes()
        .into_iter()
        .chain(image.height().to_le_bytes())
        .chain(image.as_bytes().iter().copied())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use image::{DynamicImage, ImageBuffer, Rgb};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-portable-verify-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn prepared_root(label: &str) -> (PathBuf, PathBuf) {
        let root = temp_root(label);
        std::fs::create_dir_all(&root).unwrap();
        let session = root.with_extension("session.sqlite3");
        portable::attach_root(&session, &root).unwrap();
        (root, session)
    }

    fn add_image(root: &Path, name: &str) -> (PathBuf, u64) {
        let source = root.join(name);
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(32, 24, Rgb([5, 10, 15])));
        image.save(&source).unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let modified = modified_seconds(&meta);
        let fingerprint = decoded_content_fingerprint(&source).unwrap();
        let conn = db::open(&portable::index_db_path(root)).unwrap();
        db::upsert_image(
            &conn,
            Path::new(name),
            Path::new(""),
            name,
            "png",
            meta.len(),
            modified,
            32,
            24,
            "",
            "",
            [5, 10, 15],
            1,
            &[1.0, 0.0, 0.0, 0.0],
        )
        .unwrap();
        db::set_content_fingerprint(&conn, Path::new(name), fingerprint).unwrap();
        (source, fingerprint)
    }

    fn cleanup(root: &Path, session: &Path) {
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(format!("{}-wal", session.display()));
        let _ = std::fs::remove_file(format!("{}-shm", session.display()));
    }

    #[test]
    fn newer_schema_is_refused_before_mutation() {
        let (root, session) = prepared_root("newer-schema");
        {
            let conn = Connection::open(portable::index_db_path(&root)).unwrap();
            conn.execute(
                "UPDATE portable_meta SET value='999' WHERE key='schema_version'",
                [],
            )
            .unwrap();
        }
        let error = preflight_existing_index(&root).unwrap_err().to_string();
        assert!(error.contains("newer than supported"));
        cleanup(&root, &session);
    }

    #[test]
    fn quick_verify_reports_missing_and_changed_sources() {
        let (root, session) = prepared_root("quick");
        let (source, _) = add_image(&root, "a.png");
        let first = verify_root(&root, VerifyOptions::default(), |_| {}).unwrap();
        assert!(first.is_clean());
        std::fs::remove_file(&source).unwrap();
        let second = verify_root(&root, VerifyOptions::default(), |_| {}).unwrap();
        assert!(second
            .problems
            .iter()
            .any(|problem| problem.kind == VerifyProblemKind::MissingSource));
        cleanup(&root, &session);
    }

    #[test]
    fn deep_verify_detects_same_metadata_content_replacement() {
        let (root, session) = prepared_root("deep");
        let (source, _) = add_image(&root, "a.png");
        let original_meta = std::fs::metadata(&source).unwrap();
        let replacement =
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(32, 24, Rgb([90, 80, 70])));
        replacement.save(&source).unwrap();
        let replacement_meta = std::fs::metadata(&source).unwrap();
        // PNG encoder output for solid images is stable enough to keep dimensions but not necessarily size.
        // Force stored filesystem metadata to current values so this test isolates fingerprint verification.
        {
            let conn = Connection::open(portable::index_db_path(&root)).unwrap();
            conn.execute(
                "UPDATE images SET size=?1, modified=?2 WHERE path='a.png'",
                params![
                    replacement_meta.len() as i64,
                    modified_seconds(&replacement_meta)
                ],
            )
            .unwrap();
        }
        let report = verify_root(
            &root,
            VerifyOptions {
                mode: VerifyMode::DeepFingerprint,
                batch_size: 1,
            },
            |_| {},
        )
        .unwrap();
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.kind == VerifyProblemKind::FingerprintMismatch));
        assert!(original_meta.len() > 0);
        cleanup(&root, &session);
    }

    #[test]
    fn unsafe_relative_path_is_reported_without_joining_outside_root() {
        let (root, session) = prepared_root("unsafe");
        let conn = Connection::open(portable::index_db_path(&root)).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF; INSERT INTO images(path, root, file_name, extension, size, modified, width, height) VALUES('../escape.png','','escape.png','png',1,1,1,1);",
        )
        .unwrap();
        let report = verify_root(&root, VerifyOptions::default(), |_| {}).unwrap();
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.kind == VerifyProblemKind::UnsafeRelativePath));
        cleanup(&root, &session);
    }
}
