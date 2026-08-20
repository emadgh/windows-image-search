use crate::{portable, portable_verify, thumbnail_cache};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DEFAULT_BATCH_SIZE: usize = 256;
const MAX_BATCH_SIZE: usize = 2048;
const MAX_REPORTED_PROBLEMS: usize = 256;
const ANN_MANIFEST_NAME: &str = "clip-cosine-v1.manifest";
const ANN_MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerivedRepairOptions {
    pub dry_run: bool,
    pub repair_thumbnails: bool,
    pub repair_ann: bool,
    pub batch_size: usize,
}

impl Default for DerivedRepairOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            repair_thumbnails: true,
            repair_ann: true,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }
}

impl DerivedRepairOptions {
    fn sanitized(self) -> Self {
        Self {
            dry_run: self.dry_run,
            repair_thumbnails: self.repair_thumbnails,
            repair_ann: self.repair_ann,
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnDerivedState {
    NotNeeded,
    Current,
    Missing,
    StaleOrCorrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DerivedRepairProblemKind {
    UnsafeRelativePath,
    MissingSource,
    SourceChanged,
    ThumbnailRebuildFailed,
    AnnRepairFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedRepairProblem {
    pub path: PathBuf,
    pub kind: DerivedRepairProblemKind,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedRepairProgress {
    pub root: PathBuf,
    pub records_checked: usize,
    pub total_records: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivedRepairReport {
    pub records_checked: usize,
    pub thumbnails_valid: usize,
    pub thumbnails_missing: usize,
    pub thumbnails_corrupt: usize,
    pub thumbnails_rebuilt: usize,
    pub source_records_skipped: usize,
    pub ann_state_before: Option<AnnDerivedState>,
    pub ann_rebuilt: bool,
    pub problem_count: usize,
    pub problems: Vec<DerivedRepairProblem>,
}

impl DerivedRepairReport {
    pub fn render_text(&self, root: &Path, options: DerivedRepairOptions) -> String {
        let mut text = String::new();
        text.push_str("Windows Image Search Portable Derived Cache Repair\n");
        text.push_str(&format!("root={}\n", root.display()));
        text.push_str(&format!("dry_run={}\n", options.dry_run));
        text.push_str(&format!("records_checked={}\n", self.records_checked));
        text.push_str(&format!("thumbnails_valid={}\n", self.thumbnails_valid));
        text.push_str(&format!("thumbnails_missing={}\n", self.thumbnails_missing));
        text.push_str(&format!("thumbnails_corrupt={}\n", self.thumbnails_corrupt));
        text.push_str(&format!("thumbnails_rebuilt={}\n", self.thumbnails_rebuilt));
        text.push_str(&format!("source_records_skipped={}\n", self.source_records_skipped));
        text.push_str(&format!("ann_state_before={:?}\n", self.ann_state_before));
        text.push_str(&format!("ann_rebuilt={}\n", self.ann_rebuilt));
        text.push_str(&format!("problem_count={}\n", self.problem_count));
        text.push_str(&format!("reported_problems={}\n", self.problems.len()));
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivedRepairSummary {
    pub roots_processed: usize,
    pub roots_unavailable: usize,
    pub reports: Vec<(PathBuf, DerivedRepairReport)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThumbnailState {
    Valid,
    Missing,
    Corrupt,
}

pub fn repair_available_roots<F>(
    roots: &[PathBuf],
    options: DerivedRepairOptions,
    mut progress: F,
) -> Result<DerivedRepairSummary>
where
    F: FnMut(DerivedRepairProgress),
{
    let mut summary = DerivedRepairSummary::default();
    for root in roots {
        if !root.is_dir() || !portable::index_db_path(root).is_file() {
            summary.roots_unavailable += 1;
            continue;
        }
        let report = repair_root(root, options, |event| progress(event))?;
        summary.roots_processed += 1;
        summary.reports.push((root.clone(), report));
    }
    Ok(summary)
}

pub fn repair_root<F>(
    root: &Path,
    options: DerivedRepairOptions,
    mut progress: F,
) -> Result<DerivedRepairReport>
where
    F: FnMut(DerivedRepairProgress),
{
    if !root.is_dir() {
        bail!("portable root is unavailable: {}", root.display());
    }
    let db_path = portable::index_db_path(root);
    if !db_path.is_file() {
        bail!("portable index does not exist: {}", db_path.display());
    }
    portable_verify::preflight_existing_index(root)?;
    let options = options.sanitized();
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable index read-only {}", db_path.display()))?;
    let total_records = conn.query_row("SELECT COUNT(*) FROM images", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let total_records = total_records.max(0) as usize;
    let mut report = DerivedRepairReport::default();

    if options.repair_thumbnails {
        repair_thumbnails(
            root,
            &conn,
            options,
            total_records,
            &mut report,
            &mut progress,
        )?;
    }

    if options.repair_ann {
        let state = inspect_ann_state(root, &conn)?;
        report.ann_state_before = Some(state);
        if !options.dry_run && matches!(state, AnnDerivedState::Missing | AnnDerivedState::StaleOrCorrupt)
        {
            drop(conn);
            match portable::refresh_ann(root) {
                Ok(rebuilt) => report.ann_rebuilt = rebuilt,
                Err(err) => push_problem(
                    &mut report,
                    PathBuf::from("ann-index"),
                    DerivedRepairProblemKind::AnnRepairFailed,
                    format!("{err:#}"),
                ),
            }
        }
    }

    Ok(report)
}

fn repair_thumbnails<F>(
    root: &Path,
    conn: &Connection,
    options: DerivedRepairOptions,
    total_records: usize,
    report: &mut DerivedRepairReport,
    progress: &mut F,
) -> Result<()>
where
    F: FnMut(DerivedRepairProgress),
{
    let mut cursor: Option<String> = None;
    loop {
        let mut stmt = conn.prepare(
            r#"
            SELECT path, size, modified
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
                ))
            },
        )?;
        let batch: Vec<_> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            break;
        }

        for (path_text, stored_size, stored_modified) in batch {
            cursor = Some(path_text.clone());
            report.records_checked += 1;
            let relative = PathBuf::from(&path_text);
            let absolute = match portable::absolute_source_path(root, &relative) {
                Ok(path) => path,
                Err(err) => {
                    report.source_records_skipped += 1;
                    push_problem(
                        report,
                        relative,
                        DerivedRepairProblemKind::UnsafeRelativePath,
                        format!("{err:#}"),
                    );
                    emit_progress(progress, root, report.records_checked, total_records);
                    continue;
                }
            };
            let metadata = match std::fs::metadata(&absolute) {
                Ok(meta) if meta.is_file() => meta,
                Ok(_) => {
                    report.source_records_skipped += 1;
                    push_problem(
                        report,
                        relative,
                        DerivedRepairProblemKind::MissingSource,
                        "source path is not a regular file".to_owned(),
                    );
                    emit_progress(progress, root, report.records_checked, total_records);
                    continue;
                }
                Err(err) => {
                    report.source_records_skipped += 1;
                    push_problem(
                        report,
                        relative,
                        DerivedRepairProblemKind::MissingSource,
                        err.to_string(),
                    );
                    emit_progress(progress, root, report.records_checked, total_records);
                    continue;
                }
            };
            let actual_modified = modified_seconds(&metadata);
            if metadata.len() != stored_size.max(0) as u64 || actual_modified != stored_modified {
                report.source_records_skipped += 1;
                push_problem(
                    report,
                    relative,
                    DerivedRepairProblemKind::SourceChanged,
                    format!(
                        "stored_size={} actual_size={} stored_modified={} actual_modified={}",
                        stored_size,
                        metadata.len(),
                        stored_modified,
                        actual_modified
                    ),
                );
                emit_progress(progress, root, report.records_checked, total_records);
                continue;
            }

            let cache_path = thumbnail_cache::cache_path_for_root(root, &absolute)?;
            let state = inspect_thumbnail(&cache_path);
            match state {
                ThumbnailState::Valid => report.thumbnails_valid += 1,
                ThumbnailState::Missing => report.thumbnails_missing += 1,
                ThumbnailState::Corrupt => report.thumbnails_corrupt += 1,
            }
            if !options.dry_run && state != ThumbnailState::Valid {
                let rebuilt = thumbnail_cache::load_or_build_for_root(root, &absolute).is_some();
                if rebuilt {
                    let current_path = thumbnail_cache::cache_path_for_root(root, &absolute)?;
                    if inspect_thumbnail(&current_path) == ThumbnailState::Valid {
                        report.thumbnails_rebuilt += 1;
                    } else {
                        push_problem(
                            report,
                            relative,
                            DerivedRepairProblemKind::ThumbnailRebuildFailed,
                            "thumbnail rebuild did not produce a readable cache entry".to_owned(),
                        );
                    }
                } else {
                    push_problem(
                        report,
                        relative,
                        DerivedRepairProblemKind::ThumbnailRebuildFailed,
                        "source decode or thumbnail write failed".to_owned(),
                    );
                }
            }
            emit_progress(progress, root, report.records_checked, total_records);
        }
    }
    Ok(())
}

fn inspect_thumbnail(path: &Path) -> ThumbnailState {
    if !path.is_file() {
        return ThumbnailState::Missing;
    }
    match image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.decode().ok())
    {
        Some(_) => ThumbnailState::Valid,
        None => ThumbnailState::Corrupt,
    }
}

fn inspect_ann_state(root: &Path, conn: &Connection) -> Result<AnnDerivedState> {
    let (expected_signature, expected_count) = ann_signature_and_count(conn)?;
    if expected_count == 0 {
        return Ok(AnnDerivedState::NotNeeded);
    }
    let ann_dir = portable::ann_dir(root);
    let manifest_path = ann_dir.join(ANN_MANIFEST_NAME);
    if !manifest_path.is_file() {
        return Ok(AnnDerivedState::Missing);
    }
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(_) => return Ok(AnnDerivedState::StaleOrCorrupt),
    };
    let mut version = None;
    let mut signature = None;
    let mut basename = None;
    let mut count = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "version" => version = value.trim().parse::<u32>().ok(),
            "signature" => signature = u64::from_str_radix(value.trim(), 16).ok(),
            "basename" => basename = Some(value.trim().to_owned()),
            "count" => count = value.trim().parse::<usize>().ok(),
            _ => {}
        }
    }
    if version != Some(ANN_MANIFEST_VERSION)
        || signature != Some(expected_signature)
        || count != Some(expected_count)
    {
        return Ok(AnnDerivedState::StaleOrCorrupt);
    }
    let Some(basename) = basename.filter(|value| !value.trim().is_empty()) else {
        return Ok(AnnDerivedState::StaleOrCorrupt);
    };
    let graph = ann_dir.join(format!("{basename}.hnsw.graph"));
    let data = ann_dir.join(format!("{basename}.hnsw.data"));
    if !graph.is_file() || !data.is_file() {
        return Ok(AnnDerivedState::StaleOrCorrupt);
    }
    Ok(AnnDerivedState::Current)
}

fn ann_signature_and_count(conn: &Connection) -> Result<(u64, usize)> {
    let mut stmt = conn.prepare(
        "SELECT rowid, path, size, modified, COALESCE(embedding_dim, 0), embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    1_u32.hash(&mut hasher);
    let mut count = 0usize;
    for row in rows {
        let (rowid, path, size, modified, dim, normalized) = row?;
        rowid.hash(&mut hasher);
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        modified.hash(&mut hasher);
        dim.hash(&mut hasher);
        normalized.hash(&mut hasher);
        count += 1;
    }
    Ok((hasher.finish(), count))
}

fn emit_progress<F>(progress: &mut F, root: &Path, records_checked: usize, total_records: usize)
where
    F: FnMut(DerivedRepairProgress),
{
    progress(DerivedRepairProgress {
        root: root.to_path_buf(),
        records_checked,
        total_records,
    });
}

fn push_problem(
    report: &mut DerivedRepairReport,
    path: PathBuf,
    kind: DerivedRepairProblemKind,
    detail: String,
) {
    report.problem_count += 1;
    if report.problems.len() < MAX_REPORTED_PROBLEMS {
        report.problems.push(DerivedRepairProblem { path, kind, detail });
    }
}

fn modified_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use image::{DynamicImage, ImageBuffer, Rgb};
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("wis-derived-repair-{label}-{}-{nonce}", std::process::id()))
    }

    fn setup_root(label: &str) -> (PathBuf, PathBuf) {
        let root = temp_root(label);
        std::fs::create_dir_all(root.join("images")).unwrap();
        std::fs::create_dir_all(portable::index_dir(&root)).unwrap();
        let source = root.join("images").join("a.png");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(32, 32, Rgb([20, 40, 60])))
            .save(&source)
            .unwrap();
        let metadata = std::fs::metadata(&source).unwrap();
        let modified = modified_seconds(&metadata);
        let relative = PathBuf::from("images").join("a.png");
        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS portable_meta(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO portable_meta(key, value) VALUES('library_id', 'repair-test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO portable_meta(key, value) VALUES('schema_version', '1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO portable_meta(key, value) VALUES('format', ?1)",
            params![portable_verify::PORTABLE_FORMAT_MARKER],
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO images(
                path, root, file_name, extension, size, modified, width, height,
                description, keywords, dominant_r, dominant_g, dominant_b
            ) VALUES(?1, '', 'a.png', 'png', ?2, ?3, 32, 32, '', '', 0, 0, 0)"#,
            params![relative.to_string_lossy().to_string(), metadata.len() as i64, modified],
        )
        .unwrap();
        db::set_embedding(&conn, &relative, &[1.0, 0.0]).unwrap();
        (root, source)
    }

    #[test]
    fn dry_run_then_repair_is_idempotent_for_missing_thumbnail_and_ann() {
        let (root, _source) = setup_root("missing");
        let dry = repair_root(
            &root,
            DerivedRepairOptions {
                dry_run: true,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(dry.thumbnails_missing, 1);
        assert_eq!(dry.thumbnails_rebuilt, 0);
        assert_eq!(dry.ann_state_before, Some(AnnDerivedState::Missing));
        assert!(!dry.ann_rebuilt);

        let repaired = repair_root(&root, DerivedRepairOptions::default(), |_| {}).unwrap();
        assert_eq!(repaired.thumbnails_rebuilt, 1);
        assert!(repaired.ann_rebuilt);

        let second = repair_root(
            &root,
            DerivedRepairOptions {
                dry_run: true,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(second.thumbnails_valid, 1);
        assert_eq!(second.thumbnails_missing, 0);
        assert_eq!(second.thumbnails_corrupt, 0);
        assert_eq!(second.ann_state_before, Some(AnnDerivedState::Current));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dry_run_detects_corrupt_thumbnail_without_deleting_it_then_repair_rebuilds() {
        let (root, source) = setup_root("corrupt-thumb");
        thumbnail_cache::load_or_build_for_root(&root, &source).unwrap();
        let cache = thumbnail_cache::cache_path_for_root(&root, &source).unwrap();
        std::fs::write(&cache, b"not a jpeg").unwrap();

        let dry = repair_root(
            &root,
            DerivedRepairOptions {
                dry_run: true,
                repair_ann: false,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(dry.thumbnails_corrupt, 1);
        assert_eq!(std::fs::read(&cache).unwrap(), b"not a jpeg");

        let repaired = repair_root(
            &root,
            DerivedRepairOptions {
                repair_ann: false,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(repaired.thumbnails_rebuilt, 1);
        assert_eq!(inspect_thumbnail(&cache), ThumbnailState::Valid);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn changed_source_is_not_repaired_implicitly() {
        let (root, source) = setup_root("changed-source");
        std::fs::write(&source, b"changed source bytes with another size").unwrap();
        let report = repair_root(
            &root,
            DerivedRepairOptions {
                repair_ann: false,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(report.source_records_skipped, 1);
        assert_eq!(report.thumbnails_rebuilt, 0);
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.kind == DerivedRepairProblemKind::SourceChanged));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn adding_embedding_after_ann_build_marks_cache_stale() {
        let (root, _source) = setup_root("stale-ann");
        let first = repair_root(
            &root,
            DerivedRepairOptions {
                repair_thumbnails: false,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert!(first.ann_rebuilt);

        let conn = db::open(&portable::index_db_path(&root)).unwrap();
        conn.execute(
            r#"INSERT INTO images(
                path, root, file_name, extension, size, modified, width, height,
                description, keywords, dominant_r, dominant_g, dominant_b
            ) VALUES('b.png', '', 'b.png', 'png', 1, 1, 1, 1, '', '', 0, 0, 0)"#,
            [],
        )
        .unwrap();
        db::set_embedding(&conn, Path::new("b.png"), &[0.0, 1.0]).unwrap();
        drop(conn);

        let dry = repair_root(
            &root,
            DerivedRepairOptions {
                dry_run: true,
                repair_thumbnails: false,
                ..DerivedRepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(dry.ann_state_before, Some(AnnDerivedState::StaleOrCorrupt));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unavailable_root_is_skipped_by_multi_root_repair() {
        let missing = temp_root("unavailable");
        let summary = repair_available_roots(
            &[missing],
            DerivedRepairOptions::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(summary.roots_processed, 0);
        assert_eq!(summary.roots_unavailable, 1);
        assert!(summary.reports.is_empty());
    }
}
