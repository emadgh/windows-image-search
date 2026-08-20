use anyhow::{bail, Context, Result};
use hnsw_rs::prelude::{AnnT, DistCosine, Hnsw, HnswIo};
use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufWriter;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

const PORTABLE_DIR_NAME: &str = ".imagesearch";
const PORTABLE_DB_NAME: &str = "index.sqlite3";
const THUMBNAIL_DIR_NAME: &str = "thumbnails";
const ANN_DIR_NAME: &str = "ann-index";
const PORTABLE_FORMAT_MARKER: &str = "windows-image-search-portable";
const PORTABLE_SCHEMA_VERSION: i64 = 1;
const DEFAULT_BATCH_SIZE: usize = 512;
const MAX_BATCH_SIZE: usize = 4096;
const THUMBNAIL_EDGE: u32 = 512;
const JPEG_QUALITY: u8 = 84;
const PORTABLE_FNV_OFFSET: u64 = 0xcbf29ce484222325;
const PORTABLE_FNV_PRIME: u64 = 0x100000001b3;
const ANN_INDEX_BASENAME: &str = "clip-cosine-v1";
const ANN_MANIFEST_NAME: &str = "clip-cosine-v1.manifest";
const ANN_MANIFEST_VERSION: u32 = 1;
const ANN_MAX_CONNECTIONS: usize = 24;
const ANN_MAX_LAYERS: usize = 16;
const ANN_EF_CONSTRUCTION: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepairMode {
    DryRun,
    Apply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepairScope {
    All,
    Thumbnails,
    Ann,
}

impl RepairScope {
    fn thumbnails(self) -> bool {
        matches!(self, Self::All | Self::Thumbnails)
    }

    fn ann(self) -> bool {
        matches!(self, Self::All | Self::Ann)
    }
}

#[derive(Clone, Copy, Debug)]
struct RepairOptions {
    mode: RepairMode,
    scope: RepairScope,
    batch_size: usize,
    prune_stale_thumbnails: bool,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            mode: RepairMode::DryRun,
            scope: RepairScope::All,
            batch_size: DEFAULT_BATCH_SIZE,
            prune_stale_thumbnails: false,
        }
    }
}

impl RepairOptions {
    fn sanitized(self) -> Self {
        Self {
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnnState {
    Current,
    Missing,
    Stale,
    Corrupt,
    EmptyCurrent,
}

#[derive(Clone, Debug, Default)]
struct RepairReport {
    library_id: String,
    records_checked: usize,
    thumbnail_current: usize,
    thumbnail_missing: usize,
    thumbnail_corrupt: usize,
    thumbnail_rebuilt: usize,
    thumbnail_failed: usize,
    thumbnail_source_missing: usize,
    thumbnail_source_state_mismatch: usize,
    stale_thumbnail_files: usize,
    stale_thumbnail_files_removed: usize,
    ann_before: Option<AnnState>,
    ann_after: Option<AnnState>,
    ann_rebuilt: bool,
}

impl RepairReport {
    fn render(&self, root: &Path, options: RepairOptions) -> String {
        let mut out = String::new();
        out.push_str("Windows Image Search Portable Derived Cache Repair\n");
        out.push_str(&format!("root={}\n", root.display()));
        out.push_str(&format!("library_id={}\n", self.library_id));
        out.push_str(&format!(
            "mode={}\n",
            match options.mode {
                RepairMode::DryRun => "dry-run",
                RepairMode::Apply => "apply",
            }
        ));
        out.push_str(&format!("records_checked={}\n", self.records_checked));
        out.push_str(&format!("thumbnail_current={}\n", self.thumbnail_current));
        out.push_str(&format!("thumbnail_missing={}\n", self.thumbnail_missing));
        out.push_str(&format!("thumbnail_corrupt={}\n", self.thumbnail_corrupt));
        out.push_str(&format!("thumbnail_rebuilt={}\n", self.thumbnail_rebuilt));
        out.push_str(&format!("thumbnail_failed={}\n", self.thumbnail_failed));
        out.push_str(&format!(
            "thumbnail_source_missing={}\n",
            self.thumbnail_source_missing
        ));
        out.push_str(&format!(
            "thumbnail_source_state_mismatch={}\n",
            self.thumbnail_source_state_mismatch
        ));
        out.push_str(&format!(
            "stale_thumbnail_files={}\n",
            self.stale_thumbnail_files
        ));
        out.push_str(&format!(
            "stale_thumbnail_files_removed={}\n",
            self.stale_thumbnail_files_removed
        ));
        if let Some(state) = self.ann_before {
            out.push_str(&format!("ann_before={}\n", ann_state_name(state)));
        }
        if let Some(state) = self.ann_after {
            out.push_str(&format!("ann_after={}\n", ann_state_name(state)));
        }
        out.push_str(&format!("ann_rebuilt={}\n", self.ann_rebuilt));
        out
    }
}

#[derive(Clone, Debug)]
struct PortableMetadata {
    library_id: String,
}

#[derive(Clone, Debug)]
struct AnnManifest {
    signature: u64,
    basename: String,
    count: usize,
}

#[derive(Clone, Debug)]
struct AnnEmbedding {
    rowid: usize,
    embedding: Vec<f32>,
}

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("Portable cache repair failed: {err:#}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let (root, options) = parse_args()?;
    let report = repair_root(&root, options, |done| {
        if done > 0 && done % options.batch_size.clamp(1, MAX_BATCH_SIZE) == 0 {
            eprintln!("thumbnail_progress={done}");
        }
    })?;
    println!("{}", report.render(&root, options));
    Ok(())
}

fn parse_args() -> Result<(PathBuf, RepairOptions)> {
    let mut options = RepairOptions::default();
    let mut root = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => options.mode = RepairMode::DryRun,
            "--apply" => options.mode = RepairMode::Apply,
            "--thumbnails-only" => options.scope = RepairScope::Thumbnails,
            "--ann-only" => options.scope = RepairScope::Ann,
            "--prune-stale-thumbnails" => options.prune_stale_thumbnails = true,
            "--help" | "-h" => {
                println!(
                    "Usage: portable-cache-repair.exe [--dry-run|--apply] [--thumbnails-only|--ann-only] [--prune-stale-thumbnails] <portable-root>"
                );
                std::process::exit(0);
            }
            value if value.starts_with("--batch-size=") => {
                let value = value.trim_start_matches("--batch-size=");
                options.batch_size = value
                    .parse::<usize>()
                    .context("--batch-size must be a positive integer")?;
            }
            value if value.starts_with('-') => bail!("unknown option: {value}"),
            value => {
                if root.is_some() {
                    bail!("only one portable root may be supplied");
                }
                root = Some(PathBuf::from(value));
            }
        }
    }
    let root = root.context("portable root is required; use --help for usage")?;
    Ok((root, options.sanitized()))
}

fn repair_root<F>(root: &Path, options: RepairOptions, mut progress: F) -> Result<RepairReport>
where
    F: FnMut(usize),
{
    let options = options.sanitized();
    let db_path = portable_db_path(root);
    let metadata = preflight(root, &db_path)?;
    let conn = open_read_only(&db_path)?;
    let mut report = RepairReport {
        library_id: metadata.library_id,
        ..RepairReport::default()
    };

    if options.scope.thumbnails() {
        repair_thumbnails(root, &conn, options, &mut report, &mut progress)?;
    }

    if options.scope.ann() {
        let before = inspect_ann(&db_path)?;
        report.ann_before = Some(before);
        if options.mode == RepairMode::Apply
            && !matches!(before, AnnState::Current | AnnState::EmptyCurrent)
        {
            rebuild_ann(&db_path)?;
            report.ann_rebuilt = true;
        }
        report.ann_after = Some(if options.mode == RepairMode::Apply {
            inspect_ann(&db_path)?
        } else {
            before
        });
    }

    Ok(report)
}

fn repair_thumbnails<F>(
    root: &Path,
    conn: &Connection,
    options: RepairOptions,
    report: &mut RepairReport,
    progress: &mut F,
) -> Result<()>
where
    F: FnMut(usize),
{
    let mut cursor: Option<String> = None;
    let mut expected_names = HashSet::new();

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
        let rows = stmt.query_map(params![cursor.as_deref(), options.batch_size as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let batch = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            break;
        }

        for (relative_text, stored_size, stored_modified) in batch {
            cursor = Some(relative_text.clone());
            report.records_checked += 1;
            let relative = PathBuf::from(relative_text);
            let source = match safe_absolute_source_path(root, &relative) {
                Ok(path) => path,
                Err(_) => {
                    report.thumbnail_source_missing += 1;
                    progress(report.records_checked);
                    continue;
                }
            };
            let source_meta = match std::fs::metadata(&source) {
                Ok(meta) if meta.is_file() => meta,
                _ => {
                    report.thumbnail_source_missing += 1;
                    progress(report.records_checked);
                    continue;
                }
            };
            if source_meta.len() != stored_size.max(0) as u64
                || modified_seconds(&source_meta) != stored_modified
            {
                report.thumbnail_source_state_mismatch += 1;
                progress(report.records_checked);
                continue;
            }

            let cache_path = portable_thumbnail_path(root, &relative, &source_meta);
            if let Some(name) = cache_path.file_name() {
                expected_names.insert(name.to_os_string());
            }
            match thumbnail_state(&cache_path) {
                ThumbnailState::Current => report.thumbnail_current += 1,
                ThumbnailState::Missing => {
                    report.thumbnail_missing += 1;
                    if options.mode == RepairMode::Apply {
                        match rebuild_thumbnail(&source, &cache_path) {
                            Ok(()) => report.thumbnail_rebuilt += 1,
                            Err(_) => report.thumbnail_failed += 1,
                        }
                    }
                }
                ThumbnailState::Corrupt => {
                    report.thumbnail_corrupt += 1;
                    if options.mode == RepairMode::Apply {
                        let _ = std::fs::remove_file(&cache_path);
                        match rebuild_thumbnail(&source, &cache_path) {
                            Ok(()) => report.thumbnail_rebuilt += 1,
                            Err(_) => report.thumbnail_failed += 1,
                        }
                    }
                }
            }
            progress(report.records_checked);
        }
    }

    let thumbnail_dir = root.join(PORTABLE_DIR_NAME).join(THUMBNAIL_DIR_NAME);
    if thumbnail_dir.is_dir() {
        for entry in std::fs::read_dir(&thumbnail_dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file()
                || !path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("jpg"))
            {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            if !expected_names.contains(name) {
                report.stale_thumbnail_files += 1;
                if options.mode == RepairMode::Apply && options.prune_stale_thumbnails {
                    if std::fs::remove_file(&path).is_ok() {
                        report.stale_thumbnail_files_removed += 1;
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThumbnailState {
    Current,
    Missing,
    Corrupt,
}

fn thumbnail_state(path: &Path) -> ThumbnailState {
    if !path.is_file() {
        return ThumbnailState::Missing;
    }
    match image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .ok()
        .and_then(|reader| reader.decode().ok())
    {
        Some(_) => ThumbnailState::Current,
        None => ThumbnailState::Corrupt,
    }
}

fn rebuild_thumbnail(source: &Path, destination: &Path) -> Result<()> {
    let image = image::ImageReader::open(source)
        .with_context(|| format!("opening source {}", source.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding source {}", source.display()))?;
    let thumb = image.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE).to_rgb8();
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("jpg.tmp");
    let file = File::create(&temporary)?;
    let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), JPEG_QUALITY);
    encoder.encode_image(&DynamicImage::ImageRgb8(thumb))?;
    if destination.exists() {
        let _ = std::fs::remove_file(destination);
    }
    std::fs::rename(&temporary, destination)?;
    Ok(())
}

fn inspect_ann(db_path: &Path) -> Result<AnnState> {
    let signature = ann_signature(db_path)?;
    let ann_dir = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(ANN_DIR_NAME);
    if !ann_dir.is_dir() {
        return Ok(AnnState::Missing);
    }
    let manifest = match load_ann_manifest(&ann_dir) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(AnnState::Corrupt),
    };
    if manifest.signature != signature {
        return Ok(AnnState::Stale);
    }
    if manifest.count == 0 {
        return Ok(AnnState::EmptyCurrent);
    }
    if !ann_dump_exists(&ann_dir, &manifest.basename) {
        return Ok(AnnState::Corrupt);
    }
    let mut loader = HnswIo::new(&ann_dir, &manifest.basename);
    match loader.load_hnsw::<f32, DistCosine>() {
        Ok(_) => Ok(AnnState::Current),
        Err(_) => Ok(AnnState::Corrupt),
    }
}

fn rebuild_ann(db_path: &Path) -> Result<()> {
    let ann_dir = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(ANN_DIR_NAME);
    if ann_dir.exists() {
        std::fs::remove_dir_all(&ann_dir)
            .with_context(|| format!("removing derived ANN cache {}", ann_dir.display()))?;
    }
    std::fs::create_dir_all(&ann_dir)?;

    let signature = ann_signature(db_path)?;
    let entries = load_ann_embeddings(db_path)?;
    if entries.is_empty() {
        store_ann_manifest(
            &ann_dir,
            &AnnManifest {
                signature,
                basename: ANN_INDEX_BASENAME.to_owned(),
                count: 0,
            },
        )?;
        return Ok(());
    }

    let hnsw = Hnsw::<f32, DistCosine>::new(
        ANN_MAX_CONNECTIONS,
        entries.len(),
        ANN_MAX_LAYERS,
        ANN_EF_CONSTRUCTION,
        DistCosine {},
    );
    let refs: Vec<(&Vec<f32>, usize)> = entries
        .iter()
        .map(|entry| (&entry.embedding, entry.rowid))
        .collect();
    hnsw.parallel_insert(&refs);
    let basename = hnsw
        .file_dump(&ann_dir, ANN_INDEX_BASENAME)
        .context("persisting repaired HNSW cache")?;
    store_ann_manifest(
        &ann_dir,
        &AnnManifest {
            signature,
            basename,
            count: entries.len(),
        },
    )?;
    Ok(())
}

fn load_ann_embeddings(db_path: &Path) -> Result<Vec<AnnEmbedding>> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, embedding, COALESCE(embedding_dim, 0), embedding_normalized FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut output = Vec::new();
    for row in rows {
        let (rowid, bytes, dimension, normalized) = row?;
        let Some(mut values) = decode_f32_vec(&bytes, dimension.max(0) as usize) else {
            continue;
        };
        if !normalized {
            normalize_in_place(&mut values);
        }
        output.push(AnnEmbedding {
            rowid: rowid.max(0) as usize,
            embedding: values,
        });
    }
    Ok(output)
}

fn ann_signature(db_path: &Path) -> Result<u64> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT rowid, path, size, modified, COALESCE(embedding_dim, 0), embedding_normalized, embedding FROM images WHERE embedding IS NOT NULL ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, bool>(5)?,
            row.get::<_, Vec<u8>>(6)?,
        ))
    })?;
    let mut hasher = DefaultHasher::new();
    2_u32.hash(&mut hasher);
    for row in rows {
        let (rowid, path, size, modified, dim, normalized, embedding) = row?;
        rowid.hash(&mut hasher);
        path.hash(&mut hasher);
        size.hash(&mut hasher);
        modified.hash(&mut hasher);
        dim.hash(&mut hasher);
        normalized.hash(&mut hasher);
        embedding.hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn load_ann_manifest(ann_dir: &Path) -> Result<AnnManifest> {
    let text = std::fs::read_to_string(ann_dir.join(ANN_MANIFEST_NAME))?;
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
    if version != Some(ANN_MANIFEST_VERSION) {
        bail!("unsupported ANN manifest version");
    }
    Ok(AnnManifest {
        signature: signature.context("ANN manifest has no signature")?,
        basename: basename
            .filter(|value| !value.is_empty())
            .context("ANN manifest has no basename")?,
        count: count.context("ANN manifest has no count")?,
    })
}

fn store_ann_manifest(ann_dir: &Path, manifest: &AnnManifest) -> Result<()> {
    std::fs::create_dir_all(ann_dir)?;
    let destination = ann_dir.join(ANN_MANIFEST_NAME);
    let temporary = destination.with_extension("manifest.tmp");
    std::fs::write(
        &temporary,
        format!(
            "version={}\nsignature={:016x}\nbasename={}\ncount={}\n",
            ANN_MANIFEST_VERSION, manifest.signature, manifest.basename, manifest.count
        ),
    )?;
    if destination.exists() {
        let _ = std::fs::remove_file(&destination);
    }
    std::fs::rename(&temporary, &destination)?;
    Ok(())
}

fn ann_dump_exists(ann_dir: &Path, basename: &str) -> bool {
    ann_dir.join(format!("{basename}.hnsw.graph")).is_file()
        && ann_dir.join(format!("{basename}.hnsw.data")).is_file()
}

fn preflight(root: &Path, db_path: &Path) -> Result<PortableMetadata> {
    if !root.is_dir() {
        bail!("portable root is unavailable: {}", root.display());
    }
    if !db_path.is_file() {
        bail!("portable index does not exist: {}", db_path.display());
    }
    let conn = open_read_only(db_path)?;
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='portable_meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        bail!("portable index has no portable_meta table; refusing repair");
    }
    let schema = meta_value(&conn, "schema_version")?
        .context("portable index has no schema_version metadata")?
        .parse::<i64>()
        .context("invalid portable schema_version")?;
    if schema <= 0 || schema > PORTABLE_SCHEMA_VERSION {
        bail!(
            "portable schema v{schema} is unsupported by this repair tool (max v{PORTABLE_SCHEMA_VERSION})"
        );
    }
    if let Some(marker) = meta_value(&conn, "format")? {
        if marker != PORTABLE_FORMAT_MARKER {
            bail!("unknown portable index format marker {marker:?}; refusing repair");
        }
    }
    let library_id = meta_value(&conn, "library_id")?
        .context("portable index has no library_id metadata")?;
    if library_id.trim().is_empty() {
        bail!("portable library_id is empty; refusing repair");
    }
    let has_images: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='images')",
        [],
        |row| row.get(0),
    )?;
    if !has_images {
        bail!("portable index has no images table; refusing repair");
    }
    Ok(PortableMetadata { library_id })
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening portable index read-only {}", path.display()))
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

fn portable_db_path(root: &Path) -> PathBuf {
    root.join(PORTABLE_DIR_NAME).join(PORTABLE_DB_NAME)
}

fn safe_absolute_source_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        bail!("unsafe portable relative path: {}", relative.display());
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe portable relative path: {}", relative.display())
            }
        }
    }
    Ok(root.join(relative))
}

fn portable_thumbnail_path(root: &Path, relative: &Path, meta: &std::fs::Metadata) -> PathBuf {
    let modified = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok());
    let modified_secs = modified.as_ref().map_or(0, |value| value.as_secs());
    let modified_nanos = modified.map_or(0, |value| value.subsec_nanos());
    let key = portable_thumbnail_key(relative, meta.len(), modified_secs, modified_nanos);
    root.join(PORTABLE_DIR_NAME)
        .join(THUMBNAIL_DIR_NAME)
        .join(format!("{key:016x}.jpg"))
}

fn portable_thumbnail_key(
    relative: &Path,
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
) -> u64 {
    let normalized = relative
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let mut hash = PORTABLE_FNV_OFFSET;
    fn write(hash: &mut u64, bytes: &[u8]) {
        for &byte in bytes {
            *hash ^= byte as u64;
            *hash = hash.wrapping_mul(PORTABLE_FNV_PRIME);
        }
    }
    write(&mut hash, normalized.as_bytes());
    write(&mut hash, &[0]);
    write(&mut hash, &size.to_le_bytes());
    write(&mut hash, &modified_secs.to_le_bytes());
    write(&mut hash, &modified_nanos.to_le_bytes());
    hash
}

fn modified_seconds(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

fn decode_f32_vec(bytes: &[u8], dimension: usize) -> Option<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return None;
    }
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    if dimension != 0 && dimension != values.len() {
        return None;
    }
    Some(values)
}

fn normalize_in_place(values: &mut [f32]) {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return;
    }
    let inverse = norm_sq.sqrt().recip();
    for value in values {
        *value *= inverse;
    }
}

fn encode_f32_vec(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn ann_state_name(state: AnnState) -> &'static str {
    match state {
        AnnState::Current => "current",
        AnnState::Missing => "missing",
        AnnState::Stale => "stale",
        AnnState::Corrupt => "corrupt",
        AnnState::EmptyCurrent => "empty-current",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-portable-repair-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn fixture(label: &str, with_embedding: bool) -> (PathBuf, PathBuf) {
        let root = temp_root(label);
        std::fs::create_dir_all(root.join(PORTABLE_DIR_NAME)).unwrap();
        let source = root.join("face.png");
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(32, 24, Rgb([5, 10, 15])));
        image.save(&source).unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let db_path = portable_db_path(&root);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE portable_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
            CREATE TABLE images (
                path TEXT PRIMARY KEY NOT NULL,
                size INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                embedding BLOB,
                embedding_dim INTEGER,
                embedding_normalized INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO portable_meta(key, value) VALUES('format', ?1), ('schema_version', '1'), ('library_id', 'test-library')",
            params![PORTABLE_FORMAT_MARKER],
        )
        .unwrap();
        let embedding = with_embedding.then(|| encode_f32_vec(&[1.0, 0.0]));
        conn.execute(
            "INSERT INTO images(path, size, modified, embedding, embedding_dim, embedding_normalized) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "face.png",
                meta.len() as i64,
                modified_seconds(&meta),
                embedding,
                if with_embedding { 2_i64 } else { 0_i64 },
                with_embedding
            ],
        )
        .unwrap();
        (root, source)
    }

    #[test]
    fn dry_run_is_non_destructive_and_apply_repairs_missing_thumbnail() {
        let (root, source) = fixture("missing-thumb", false);
        let dry = repair_root(
            &root,
            RepairOptions {
                scope: RepairScope::Thumbnails,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(dry.thumbnail_missing, 1);
        let cache = portable_thumbnail_path(&root, Path::new("face.png"), &std::fs::metadata(&source).unwrap());
        assert!(!cache.exists());

        let applied = repair_root(
            &root,
            RepairOptions {
                mode: RepairMode::Apply,
                scope: RepairScope::Thumbnails,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(applied.thumbnail_rebuilt, 1);
        assert!(cache.is_file());

        let second = repair_root(
            &root,
            RepairOptions {
                mode: RepairMode::Apply,
                scope: RepairScope::Thumbnails,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(second.thumbnail_current, 1);
        assert_eq!(second.thumbnail_rebuilt, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_thumbnail_is_reported_and_rebuilt() {
        let (root, source) = fixture("corrupt-thumb", false);
        let cache = portable_thumbnail_path(&root, Path::new("face.png"), &std::fs::metadata(&source).unwrap());
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, b"not a jpeg").unwrap();

        let dry = repair_root(
            &root,
            RepairOptions {
                scope: RepairScope::Thumbnails,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(dry.thumbnail_corrupt, 1);

        let applied = repair_root(
            &root,
            RepairOptions {
                mode: RepairMode::Apply,
                scope: RepairScope::Thumbnails,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(applied.thumbnail_rebuilt, 1);
        assert_eq!(thumbnail_state(&cache), ThumbnailState::Current);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ann_missing_stale_and_corrupt_states_are_recoverable_and_idempotent() {
        let (root, _) = fixture("ann", true);
        let db_path = portable_db_path(&root);
        assert_eq!(inspect_ann(&db_path).unwrap(), AnnState::Missing);

        let first = repair_root(
            &root,
            RepairOptions {
                mode: RepairMode::Apply,
                scope: RepairScope::Ann,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert!(first.ann_rebuilt);
        assert_eq!(first.ann_after, Some(AnnState::Current));

        let second = repair_root(
            &root,
            RepairOptions {
                mode: RepairMode::Apply,
                scope: RepairScope::Ann,
                ..RepairOptions::default()
            },
            |_| {},
        )
        .unwrap();
        assert!(!second.ann_rebuilt);
        assert_eq!(second.ann_before, Some(AnnState::Current));

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE images SET embedding = ?1 WHERE path = 'face.png'",
                params![encode_f32_vec(&[0.0, 1.0])],
            )
            .unwrap();
        }
        assert_eq!(inspect_ann(&db_path).unwrap(), AnnState::Stale);
        rebuild_ann(&db_path).unwrap();
        assert_eq!(inspect_ann(&db_path).unwrap(), AnnState::Current);

        let manifest = load_ann_manifest(&root.join(PORTABLE_DIR_NAME).join(ANN_DIR_NAME)).unwrap();
        let graph = root
            .join(PORTABLE_DIR_NAME)
            .join(ANN_DIR_NAME)
            .join(format!("{}.hnsw.graph", manifest.basename));
        std::fs::remove_file(graph).unwrap();
        assert_eq!(inspect_ann(&db_path).unwrap(), AnnState::Corrupt);
        rebuild_ann(&db_path).unwrap();
        assert_eq!(inspect_ann(&db_path).unwrap(), AnnState::Current);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unavailable_root_is_refused_without_scanning() {
        let root = temp_root("missing");
        let error = repair_root(&root, RepairOptions::default(), |_| {}).unwrap_err();
        assert!(format!("{error:#}").contains("unavailable"));
    }

    #[test]
    fn thumbnail_key_matches_portable_cache_contract() {
        assert_eq!(
            portable_thumbnail_key(Path::new("tiles/stone/face.jpg"), 12345, 55, 9),
            0x0a916e50a289f87c
        );
    }
}
