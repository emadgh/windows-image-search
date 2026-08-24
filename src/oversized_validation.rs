use crate::db::{self, DescriptorSource};
use crate::embedding::EmbeddingService;
use crate::indexer::{self, IndexControl, WorkerMessage};
use crate::oversized_preview;
use crate::portable;
use crate::settings::{
    ClipExecutionProvider, IndexingSettings, DIRECT_DECODE_MAX_FILE_SIZE_BYTES,
    DIRECT_DECODE_MAX_FILE_SIZE_MIB, MAX_FILE_SIZE_MIB,
};
use anyhow::{bail, Context, Result};
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceState {
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
}

struct ValidationWorkspace {
    root: PathBuf,
    linked_source: PathBuf,
    session_db: PathBuf,
}

impl ValidationWorkspace {
    fn new(source: &Path) -> Result<(Self, PathBuf, SourceState)> {
        let source = std::fs::canonicalize(source)
            .with_context(|| format!("resolving validation source {}", source.display()))?;
        let state = source_state(&source)?;
        validate_source(&source, state.size)?;

        let parent = source
            .parent()
            .context("oversized validation source has no parent directory")?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = parent.join(format!(
            ".wis-oversized-validation-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root)
            .with_context(|| format!("creating validation root {}", root.display()))?;

        let file_name = source
            .file_name()
            .context("oversized validation source has no file name")?;
        let linked_source = root.join(file_name);
        if let Err(err) = std::fs::hard_link(&source, &linked_source) {
            let _ = std::fs::remove_dir_all(&root);
            bail!(
                "cannot create a same-volume hard link for isolated validation of {}: {err}. Move/copy the test image to an NTFS volume that supports hard links and retry",
                source.display()
            );
        }

        let linked_state = source_state(&linked_source)?;
        if linked_state != state {
            let _ = std::fs::remove_dir_all(&root);
            bail!("validation hard link does not preserve source state");
        }

        let session_db = root.join("validation-session.sqlite3");
        Ok((
            Self {
                root,
                linked_source,
                session_db,
            },
            source,
            state,
        ))
    }
}

impl Drop for ValidationWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn validate_preview(source: &Path) -> Result<String> {
    let (workspace, original, original_state) = ValidationWorkspace::new(source)?;
    let started = Instant::now();

    let cold_started = Instant::now();
    let cold = oversized_preview::load_current_for_root(&workspace.root, &workspace.linked_source)?;
    let cold_elapsed = cold_started.elapsed();
    if cold.reused {
        bail!("fresh validation workspace unexpectedly reused an oversized preview");
    }
    if cold.image.width() > oversized_preview::PREVIEW_EDGE
        || cold.image.height() > oversized_preview::PREVIEW_EDGE
    {
        bail!(
            "generated preview exceeds the configured {} px edge",
            oversized_preview::PREVIEW_EDGE
        );
    }
    let preview_meta = std::fs::metadata(&cold.path)
        .with_context(|| format!("reading generated preview {}", cold.path.display()))?;

    let warm_started = Instant::now();
    let warm = oversized_preview::load_current_for_root(&workspace.root, &workspace.linked_source)?;
    let warm_elapsed = warm_started.elapsed();
    if !warm.reused {
        bail!("second oversized preview load regenerated instead of reusing the derivative");
    }
    if warm.path != cold.path {
        bail!("warm validation resolved a different derivative cache identity");
    }

    ensure_source_unchanged(&original, original_state)?;

    let mut report = String::new();
    report.push_str("Windows Image Search Oversized Preview Validation\n");
    append_source_report(&mut report, &original, original_state);
    report.push_str(&format!(
        "source_dimensions={}x{}\n",
        cold.source_width, cold.source_height
    ));
    report.push_str(&format!(
        "direct_decode_ceiling_mib={}\n",
        DIRECT_DECODE_MAX_FILE_SIZE_MIB
    ));
    report.push_str(&format!(
        "preview_revision={}\npreview_edge={}\n",
        oversized_preview::PREVIEW_REVISION,
        oversized_preview::PREVIEW_EDGE
    ));
    report.push_str(&format!(
        "preview_dimensions={}x{}\npreview_file_bytes={}\n",
        cold.image.width(),
        cold.image.height(),
        preview_meta.len()
    ));
    report.push_str(&format!("cold_reused={}\n", cold.reused));
    report.push_str(&format!("cold_wall_ms={}\n", cold_elapsed.as_millis()));
    report.push_str(&format!("warm_reused={}\n", warm.reused));
    report.push_str(&format!("warm_wall_ms={}\n", warm_elapsed.as_millis()));
    report.push_str("source_state_unchanged=true\n");
    report.push_str(&format!(
        "total_wall_ms={}\n",
        started.elapsed().as_millis()
    ));
    report.push_str("validation_passed=true\n");
    Ok(report)
}

pub fn validate_indexing(source: &Path, model_cache: &Path) -> Result<String> {
    let (workspace, original, original_state) = ValidationWorkspace::new(source)?;
    let max_file_size_mib = required_allowance_mib(original_state.size)?;
    let settings = IndexingSettings {
        decode_workers: 1,
        clip_threads: 1,
        batch_size: 1,
        clip_execution_provider: ClipExecutionProvider::Cpu,
        max_file_size_mib,
    }
    .sanitized();

    portable::attach_root(&workspace.session_db, &workspace.root)
        .context("preparing isolated portable validation root")?;

    let embedding_service = EmbeddingService::new(model_cache.to_path_buf());
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    indexer::spawn_rescan(
        workspace.session_db.clone(),
        vec![workspace.root.clone()],
        settings,
        embedding_service,
        IndexControl::default(),
        tx,
    );

    let mut warnings = Vec::<String>::new();
    let mut statuses = Vec::<String>::new();
    loop {
        if started.elapsed() > VALIDATION_TIMEOUT {
            bail!(
                "oversized indexing validation exceeded {} minutes",
                VALIDATION_TIMEOUT.as_secs() / 60
            );
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(WorkerMessage::Status(status)) => {
                eprintln!("validation_status={status}");
                statuses.push(status);
            }
            Ok(WorkerMessage::CurrentFile(path)) => {
                eprintln!("validation_file={path}");
            }
            Ok(WorkerMessage::Warning(warning)) => {
                eprintln!("validation_warning={warning}");
                warnings.push(warning);
            }
            Ok(WorkerMessage::Error(error)) => bail!("indexing worker failed: {error}"),
            Ok(WorkerMessage::Idle) => break,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                bail!("indexing worker disconnected before reporting Idle")
            }
        }
    }

    let relative = portable::relative_source_path(&workspace.root, &workspace.linked_source)?;
    let conn = db::open(&portable::index_db_path(&workspace.root))?;
    let provenance = db::descriptor_provenance(&conn, &relative)?
        .context("indexed validation record has no descriptor provenance")?;
    if provenance.source != DescriptorSource::OversizedPreview {
        bail!(
            "oversized validation record used {:?} instead of OversizedPreview",
            provenance.source
        );
    }
    if provenance.preview_revision != Some(oversized_preview::PREVIEW_REVISION as u32)
        || provenance.preview_edge != Some(oversized_preview::PREVIEW_EDGE)
    {
        bail!("indexed validation record has unexpected preview revision/edge provenance");
    }

    let row = conn.query_row(
        "SELECT size, modified, width, height, COALESCE(embedding_dim, 0), length(embedding), \
                COALESCE(color_histogram_dim, 0), COALESCE(material_texture_dim, 0) \
         FROM images WHERE path = ?1",
        params![relative.to_string_lossy().to_string()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        },
    )?;
    let (
        stored_size,
        stored_modified,
        width,
        height,
        embedding_dim,
        embedding_bytes,
        histogram_dim,
        texture_dim,
    ) = row;
    if stored_size.max(0) as u64 != original_state.size {
        bail!("indexed record did not preserve the authoritative original source size");
    }
    let expected_modified = i64::try_from(original_state.modified_secs)
        .context("source modified time does not fit persisted index metadata")?;
    if stored_modified != expected_modified {
        bail!(
            "indexed record did not preserve authoritative source mtime: expected={expected_modified} actual={stored_modified}"
        );
    }
    if width <= 0 || height <= 0 {
        bail!("indexed record has invalid original dimensions {width}x{height}");
    }
    if embedding_dim <= 0 || embedding_bytes != Some(embedding_dim.saturating_mul(4)) {
        bail!(
            "CLIP embedding is missing or malformed: dim={embedding_dim} bytes={embedding_bytes:?}"
        );
    }
    if histogram_dim <= 0 || texture_dim <= 0 {
        bail!(
            "visual descriptor dimensions are incomplete: histogram={histogram_dim} texture={texture_dim}"
        );
    }

    let preview =
        oversized_preview::load_current_for_root(&workspace.root, &workspace.linked_source)?;
    if !preview.reused {
        bail!("post-index validation could not reuse the generated oversized derivative");
    }
    if width as u32 != preview.source_width || height as u32 != preview.source_height {
        bail!(
            "indexed dimensions {width}x{height} do not match authoritative source dimensions {}x{}",
            preview.source_width,
            preview.source_height
        );
    }
    ensure_source_unchanged(&original, original_state)?;

    let mut report = String::new();
    report.push_str("Windows Image Search Oversized Full-Index Validation\n");
    append_source_report(&mut report, &original, original_state);
    report.push_str(&format!("configured_allowance_mib={max_file_size_mib}\n"));
    report.push_str("clip_provider=CPU\n");
    report.push_str(&format!("indexed_modified_secs={stored_modified}\n"));
    report.push_str(&format!("indexed_dimensions={}x{}\n", width, height));
    report.push_str("descriptor_source=OversizedPreview\n");
    report.push_str(&format!(
        "preview_revision={}\npreview_edge={}\n",
        provenance.preview_revision.unwrap_or_default(),
        provenance.preview_edge.unwrap_or_default()
    ));
    report.push_str(&format!("embedding_dim={embedding_dim}\n"));
    report.push_str(&format!("color_histogram_dim={histogram_dim}\n"));
    report.push_str(&format!("material_texture_dim={texture_dim}\n"));
    report.push_str(&format!("preview_reused_after_index={}\n", preview.reused));
    report.push_str(&format!("warnings={}\n", warnings.len()));
    report.push_str(&format!("status_updates={}\n", statuses.len()));
    report.push_str("source_state_unchanged=true\n");
    report.push_str(&format!(
        "total_wall_ms={}\n",
        started.elapsed().as_millis()
    ));
    report.push_str("validation_passed=true\n");
    for (index, warning) in warnings.iter().enumerate() {
        report.push_str(&format!("warning_{}={}\n", index + 1, single_line(warning)));
    }
    Ok(report)
}

fn validate_source(path: &Path, size: u64) -> Result<()> {
    if !path.is_file() {
        bail!(
            "validation source is not a regular file: {}",
            path.display()
        );
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "jpg" && extension != "jpeg" {
        bail!("oversized validation currently requires a JPEG source");
    }
    if size <= DIRECT_DECODE_MAX_FILE_SIZE_BYTES {
        bail!(
            "validation source is {} MiB; use a file larger than the {} MiB direct-decode ceiling",
            bytes_to_mib(size),
            DIRECT_DECODE_MAX_FILE_SIZE_MIB
        );
    }
    let required = required_allowance_mib(size)?;
    if required > MAX_FILE_SIZE_MIB {
        bail!(
            "validation source requires {required} MiB allowance, above the supported {MAX_FILE_SIZE_MIB} MiB maximum"
        );
    }
    Ok(())
}

fn required_allowance_mib(size: u64) -> Result<usize> {
    let mib = size.saturating_add(MIB - 1) / MIB;
    usize::try_from(mib).context("source size does not fit configured allowance")
}

fn source_state(path: &Path) -> Result<SourceState> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("reading validation source metadata {}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok());
    Ok(SourceState {
        size: meta.len(),
        modified_secs: modified.as_ref().map_or(0, |value| value.as_secs()),
        modified_nanos: modified.map_or(0, |value| value.subsec_nanos()),
    })
}

fn ensure_source_unchanged(path: &Path, before: SourceState) -> Result<()> {
    let after = source_state(path)?;
    if before != after {
        bail!("validation changed authoritative source state: before={before:?} after={after:?}");
    }
    Ok(())
}

fn append_source_report(report: &mut String, source: &Path, state: SourceState) {
    report.push_str(&format!("source={}\n", source.display()));
    report.push_str(&format!("source_size_bytes={}\n", state.size));
    report.push_str(&format!(
        "source_size_mib={:.2}\n",
        bytes_to_mib(state.size)
    ));
    report.push_str(&format!("source_modified_secs={}\n", state.modified_secs));
    report.push_str(&format!("source_modified_nanos={}\n", state.modified_nanos));
}

fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n', '\t'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowance_rounds_up_at_mib_boundary() {
        assert_eq!(required_allowance_mib(256 * MIB + 1).unwrap(), 257);
        assert_eq!(required_allowance_mib(640 * MIB).unwrap(), 640);
        assert_eq!(required_allowance_mib(640 * MIB + 1).unwrap(), 641);
    }

    #[test]
    fn single_line_diagnostics_remove_control_separators() {
        assert_eq!(single_line("a\tb\r\nc"), "a b  c");
    }
}
