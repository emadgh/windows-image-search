use crate::ann;
use crate::db::{self, ImageRecord, ImageSummary};
use crate::embedding::EmbeddingService;
use crate::material_texture;
use crate::metadata;
use crate::settings::{ClipExecutionProvider, IndexingSettings};
use crate::thumbnail_cache;
use anyhow::{bail, Context, Result};
use image::{imageops::FilterType, DynamicImage, GenericImageView, Pixel};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const COLOR_HISTOGRAM_BINS: usize = 64;
const MAX_SIMILARITY_RESULTS: usize = 2_000;
const CANDIDATE_PIPELINE_MIN_RECORDS: usize = 4_000;
const MAX_COMPONENT_CANDIDATES: usize = 3_000;

#[derive(Clone)]
struct PendingImage {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: i64,
}

struct PreparedImage {
    root: PathBuf,
    path: PathBuf,
    file_name: String,
    extension: String,
    size: u64,
    modified: i64,
    width: u32,
    height: u32,
    description: String,
    keywords: String,
    dominant: [u8; 3],
    visual_hash: u64,
    color_histogram: Vec<f32>,
    material_texture: Vec<f32>,
}

impl PreparedImage {
    fn to_summary(&self) -> ImageSummary {
        ImageSummary {
            path: self.path.clone(),
            root: self.root.clone(),
            file_name: self.file_name.clone(),
            extension: self.extension.clone(),
            size: self.size,
            modified: self.modified,
            width: self.width,
            height: self.height,
            description: self.description.clone(),
            keywords: self.keywords.clone(),
            dominant: self.dominant,
            score: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SimilaritySettings {
    pub color_distribution_weight: f32,
    pub texture_weight: f32,
    pub clip_weight: f32,
    pub dominant_color_weight: f32,
    pub strict_color_rejection: bool,
    pub min_color_distribution_match: f32,
    pub max_dominant_color_difference: f32,
}

impl Default for SimilaritySettings {
    fn default() -> Self {
        Self {
            color_distribution_weight: 44.0,
            texture_weight: 31.0,
            clip_weight: 20.0,
            dominant_color_weight: 5.0,
            strict_color_rejection: true,
            min_color_distribution_match: 30.0,
            max_dominant_color_difference: 30.0,
        }
    }
}

#[derive(Debug)]
pub enum WorkerMessage {
    Status(String),
    Progress { done: usize, total: usize },
    Reload,
    IndexedBatch(Vec<ImageSummary>),
    RemovedPaths(Vec<PathBuf>),
    SimilarityResults(Vec<ImageSummary>),
    Error(String),
    Idle,
}

pub fn spawn_rescan(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &roots, indexing_settings, &embedding_service, &tx);
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!("Indexing failed: {err:#}")));
        }
        let _ = tx.send(WorkerMessage::Reload);
        let _ = tx.send(WorkerMessage::Idle);
    });
}

pub fn spawn_incremental_update(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    changed_paths: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = incremental_update(
            &db_path,
            &roots,
            &changed_paths,
            indexing_settings,
            &embedding_service,
            &tx,
        );
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Live indexing failed: {err:#}"
            )));
        }
        let _ = tx.send(WorkerMessage::Idle);
    });
}

fn incremental_update(
    db_path: &Path,
    roots: &[PathBuf],
    changed_paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);
    let mut conn = db::open(db_path)?;
    let existing_states = db::load_file_states(&conn)?;
    let unique_paths: HashSet<PathBuf> = changed_paths.iter().cloned().collect();
    let mut candidates = HashMap::<PathBuf, PathBuf>::new();
    let mut removed_targets = Vec::<PathBuf>::new();

    for changed in unique_paths {
        let Some(root) = indexed_root_for_path(&changed, roots) else {
            continue;
        };
        if changed.exists() {
            if changed.is_file() {
                if is_supported_image(&changed) {
                    candidates.insert(changed, root.clone());
                }
            } else if changed.is_dir() {
                for entry in WalkDir::new(&changed).follow_links(false).into_iter() {
                    match entry {
                        Ok(entry)
                            if entry.file_type().is_file() && is_supported_image(entry.path()) =>
                        {
                            candidates.insert(entry.into_path(), root.clone());
                        }
                        Ok(_) => {}
                        Err(err) => {
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Live subtree scan could not access {}: {err}",
                                changed.display()
                            )));
                        }
                    }
                }
            }
        } else {
            removed_targets.push(changed);
        }
    }

    let mut removed_paths = Vec::<PathBuf>::new();
    for target in removed_targets {
        removed_paths.extend(db::delete_path_tree(&conn, &target)?);
    }
    removed_paths.sort();
    removed_paths.dedup();
    if !removed_paths.is_empty() {
        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));
    }

    let mut pending = Vec::<PendingImage>::new();
    for (path, root) in candidates {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let unchanged = existing_states
            .get(&path)
            .is_some_and(|state| state.size == size && state.modified == modified);
        if !unchanged {
            pending.push(PendingImage {
                root,
                path,
                size,
                modified,
            });
        }
    }

    if pending.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Live index synchronized: 0 changed, {} removed",
            removed_paths.len()
        )));
        return Ok(());
    }

    let workers = indexing_settings.decode_workers;
    let batch_size = indexing_settings.batch_size;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("live-image-index-{index}"))
        .build()
        .context("creating live image indexing worker pool")?;
    let mut committed_paths = Vec::<PathBuf>::new();
    let total = pending.len();
    let done = AtomicUsize::new(0);

    for batch in pending.chunks(batch_size) {
        let prepared: Vec<PreparedImage> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|item| {
                    let result = inspect_image(&item.path, &thumbnail_cache_dir).map(
                        |(
                            width,
                            height,
                            dominant,
                            visual_hash,
                            color_histogram,
                            material_texture,
                        )| {
                            let text = metadata::extract(&item.path);
                            PreparedImage {
                                root: item.root.clone(),
                                path: item.path.clone(),
                                file_name: item
                                    .path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or_default()
                                    .to_owned(),
                                extension: item
                                    .path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .unwrap_or_default()
                                    .to_ascii_lowercase(),
                                size: item.size,
                                modified: item.modified,
                                width,
                                height,
                                description: text.description,
                                keywords: text.keywords,
                                dominant,
                                visual_hash,
                                color_histogram,
                                material_texture,
                            }
                        },
                    );
                    let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if current == total || current % 8 == 0 {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Live indexing changed files: {current}/{total}"
                        )));
                    }
                    match result {
                        Ok(value) => Some(value),
                        Err(err) => {
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Cannot decode changed image {}: {err:#}",
                                item.path.display()
                            )));
                            None
                        }
                    }
                })
                .collect()
        });

        if prepared.is_empty() {
            continue;
        }
        {
            let transaction = conn.transaction()?;
            for item in &prepared {
                db::upsert_image(
                    &transaction,
                    &item.path,
                    &item.root,
                    &item.file_name,
                    &item.extension,
                    item.size,
                    item.modified,
                    item.width,
                    item.height,
                    &item.description,
                    &item.keywords,
                    item.dominant,
                    item.visual_hash,
                    &item.color_histogram,
                )?;
                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;
                committed_paths.push(item.path.clone());
            }
            transaction.commit()?;
        }
        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();
        let _ = tx.send(WorkerMessage::IndexedBatch(live_records));
    }

    if !committed_paths.is_empty() {
        if let Err(err) = build_embeddings(
            &mut conn,
            &committed_paths,
            indexing_settings,
            embedding_service,
            tx,
        ) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Live metadata/visual index is ready, but CLIP update failed: {err:#}"
            )));
        }
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Live index synchronized: {} changed, {} removed",
        committed_paths.len(),
        removed_paths.len()
    )));
    Ok(())
}

fn indexed_root_for_path<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn rescan(
    db_path: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);
    let mut conn = db::open(db_path)?;
    let existing_file_states = db::load_file_states(&conn)?;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();

    let _ = tx.send(WorkerMessage::Status(format!(
        "Scanning folders recursively… {} persisted file states cached in memory",
        existing_file_states.len()
    )));
    let mut traversal_errors = 0usize;
    let mut prunable_roots = Vec::<PathBuf>::new();
    for root in roots {
        if !root.exists() {
            traversal_errors += 1;
            let _ = tx.send(WorkerMessage::Error(format!(
                "Indexed root does not exist; stale cleanup skipped for {}",
                root.display()
            )));
            continue;
        }

        let root_errors_before = traversal_errors;
        for entry in WalkDir::new(root).follow_links(false).into_iter() {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() && is_supported_image(entry.path()) {
                        candidates.push((root.clone(), entry.into_path()));
                    }
                }
                Err(err) => {
                    traversal_errors += 1;
                    if traversal_errors <= 8 {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Recursive scan could not access an entry under {}: {err}",
                            root.display()
                        )));
                    }
                }
            }
        }

        if traversal_errors == root_errors_before {
            prunable_roots.push(root.clone());
        } else {
            let _ = tx.send(WorkerMessage::Status(format!(
                "Stale cleanup skipped for {} because traversal was incomplete",
                root.display()
            )));
        }
    }

    let total = candidates.len();
    let mut pending = Vec::<PendingImage>::new();

    // Keep filesystem/SQLite state checks cheap and serialized, but move image
    // decoding + metadata extraction to a small worker pool below. Completed
    // batches are committed immediately so a crash does not discard the whole run.
    for (index, (root, path)) in candidates.iter().enumerate() {
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = tx.send(WorkerMessage::Error(format!(
                    "Cannot read {}: {err}",
                    path.display()
                )));
                continue;
            }
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);

        let unchanged = existing_file_states
            .get(path)
            .is_some_and(|state| state.size == size && state.modified == modified);

        if !unchanged {
            pending.push(PendingImage {
                root: root.clone(),
                path: path.clone(),
                size,
                modified,
            });
        }

        if index % 32 == 0 || index + 1 == total {
            let _ = tx.send(WorkerMessage::Progress {
                done: index + 1,
                total,
            });
        }
    }

    let changed_total = pending.len();
    let workers = indexing_settings.decode_workers;
    let batch_size = indexing_settings.batch_size;
    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} changed image{} with {workers} decode worker{}; committing every {batch_size} images…",
        if changed_total == 1 { "" } else { "s" },
        if workers == 1 { "" } else { "s" },
    )));

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("image-index-{index}"))
        .build()
        .context("creating image indexing worker pool")?;
    let prepared_count = AtomicUsize::new(0);
    let mut changed = 0usize;

    for batch in pending.chunks(batch_size) {
        let prepared: Vec<PreparedImage> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|item| {
                    let result = inspect_image(&item.path, &thumbnail_cache_dir).map(
                        |(
                            width,
                            height,
                            dominant,
                            visual_hash,
                            color_histogram,
                            material_texture,
                        )| {
                            let text = metadata::extract(&item.path);
                            PreparedImage {
                                root: item.root.clone(),
                                path: item.path.clone(),
                                file_name: item
                                    .path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or_default()
                                    .to_owned(),
                                extension: item
                                    .path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .unwrap_or_default()
                                    .to_ascii_lowercase(),
                                size: item.size,
                                modified: item.modified,
                                width,
                                height,
                                description: text.description,
                                keywords: text.keywords,
                                dominant,
                                visual_hash,
                                color_histogram,
                                material_texture,
                            }
                        },
                    );

                    let done = prepared_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if done % 8 == 0 || done == changed_total {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Decoding/reading metadata: {done}/{changed_total}; committed {changed}"
                        )));
                    }

                    match result {
                        Ok(value) => Some(value),
                        Err(err) => {
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Cannot decode {}: {err:#}",
                                item.path.display()
                            )));
                            None
                        }
                    }
                })
                .collect()
        });

        if prepared.is_empty() {
            continue;
        }

        {
            let transaction = conn.transaction()?;
            for item in &prepared {
                db::upsert_image(
                    &transaction,
                    &item.path,
                    &item.root,
                    &item.file_name,
                    &item.extension,
                    item.size,
                    item.modified,
                    item.width,
                    item.height,
                    &item.description,
                    &item.keywords,
                    item.dominant,
                    item.visual_hash,
                    &item.color_histogram,
                )?;
                db::set_material_texture(&transaction, &item.path, &item.material_texture)?;
            }
            transaction.commit()?;
        }

        changed += prepared.len();
        let live_records = prepared.iter().map(PreparedImage::to_summary).collect();
        let _ = tx.send(WorkerMessage::IndexedBatch(live_records));
        let _ = tx.send(WorkerMessage::Status(format!(
            "Committed base index: {changed}/{changed_total} changed images safely stored"
        )));
    }

    let scan_generation = db::next_scan_generation(&conn)?;
    let marked = db::mark_paths_seen(
        &mut conn,
        scan_generation,
        candidates.iter().map(|(_, path)| path),
    )?;
    let mut removed = 0usize;
    for root in &prunable_roots {
        removed += db::delete_stale_for_root(&conn, root, scan_generation)?;
    }
    let _ = tx.send(WorkerMessage::Status(format!(
        "Scan generation {scan_generation}: {marked} persisted paths marked present; {removed} stale rows removed across {}/{} safe roots",
        prunable_roots.len(),
        roots.len()
    )));

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading visual index: {} image{} need texture/color descriptors…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, indexing_settings.decode_workers, tx)?;
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Base index updated: {changed} changed, {removed} removed. Preparing CLIP embeddings…"
    )));

    let missing = db::paths_missing_embedding(&conn)?;
    if !missing.is_empty() {
        if let Err(err) = build_embeddings(
            &mut conn,
            &missing,
            indexing_settings,
            embedding_service,
            tx,
        ) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Texture/color index is ready, but CLIP indexing is unavailable: {err:#}"
            )));
        }
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Index ready: {total} image{} (recursive scan, {traversal_errors} traversal error{})",
        if total == 1 { "" } else { "s" },
        if traversal_errors == 1 { "" } else { "s" }
    )));
    Ok(())
}

fn build_visual_descriptors(
    conn: &rusqlite::Connection,
    paths: &[PathBuf],
    workers: usize,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .thread_name(|index| format!("visual-index-{index}"))
        .build()
        .context("creating visual descriptor worker pool")?;
    let done = AtomicUsize::new(0);

    let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>)> = pool.install(|| {
        paths
            .par_iter()
            .filter_map(|path| {
                let result = decode_image(path).map(|image| visual_descriptor(&image));
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current % 25 == 0 || current == total {
                    let _ = tx.send(WorkerMessage::Status(format!(
                        "Building texture/color index: {current}/{total}"
                    )));
                }
                match result {
                    Ok((_, visual_hash, color_histogram, material_texture)) => {
                        Some((path.clone(), visual_hash, color_histogram, material_texture))
                    }
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Cannot build visual descriptor for {}: {err:#}",
                            path.display()
                        )));
                        None
                    }
                }
            })
            .collect()
    });

    for (path, visual_hash, color_histogram, material_texture) in descriptors {
        db::set_visual_descriptor(conn, &path, visual_hash, &color_histogram)?;
        db::set_material_texture(conn, &path, &material_texture)?;
    }
    Ok(())
}

fn build_embeddings(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let _ = tx.send(WorkerMessage::Status(
        "Using persistent CLIP embedding service…".to_owned(),
    ));

    let total = paths.len();
    let batch_size = indexing_settings.batch_size;
    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {
        let response = embedding_service
            .embed_with_provider(
                batch.to_vec(),
                batch_size,
                indexing_settings.clip_threads,
                indexing_settings.clip_execution_provider,
            )
            .with_context(|| format!("embedding image batch {}", batch_index + 1))?;
        if response.embeddings.len() != batch.len() {
            bail!(
                "CLIP returned {} embeddings for {} input images",
                response.embeddings.len(),
                batch.len()
            );
        }
        if batch_index == 0 {
            let mut status = if response.model_reloaded {
                format!(
                    "CLIP model initialized on {} with {} CPU thread{}; subsequent batches/searches will reuse it",
                    response.active_provider.label(),
                    indexing_settings.clip_threads,
                    if indexing_settings.clip_threads == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "Reusing the already-loaded CLIP model on {}",
                    response.active_provider.label()
                )
            };
            if let Some(reason) = &response.fallback_reason {
                status.push_str(&format!(" — {reason}"));
            }
            let _ = tx.send(WorkerMessage::Status(status));
        }

        {
            let transaction = conn.transaction()?;
            for (path, embedding) in batch.iter().zip(response.embeddings.iter()) {
                db::set_embedding(&transaction, path, embedding)?;
            }
            transaction.commit()?;
        }
        let done = ((batch_index + 1) * batch_size).min(total);
        let _ = tx.send(WorkerMessage::Status(format!(
            "Building CLIP index: {done}/{total} (committed; persistent model)"
        )));
    }
    Ok(())
}

pub fn spawn_similarity_search(
    db_path: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let _ = tx.send(WorkerMessage::Status(
            "Preparing hybrid visual search…".to_owned(),
        ));
        match similarity_search(
            &db_path,
            &query_path,
            settings,
            indexing_settings,
            &embedding_service,
            &tx,
        ) {
            Ok(results) => {
                let count = results.len();
                let _ = tx.send(WorkerMessage::SimilarityResults(results));
                let _ = tx.send(WorkerMessage::Status(format!(
                    "Hybrid visual search complete: {count} matches"
                )));
            }
            Err(err) => {
                let _ = tx.send(WorkerMessage::Error(format!(
                    "Similarity search failed: {err:#}"
                )));
            }
        }
        let _ = tx.send(WorkerMessage::Idle);
    });
}

#[derive(Clone, Copy, Debug)]
struct SimilarityMetrics {
    index: usize,
    is_exact: bool,
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    passes_color_gate: bool,
}

fn component_candidate_limit(record_count: usize) -> usize {
    record_count
        .div_ceil(2)
        .clamp(MAX_SIMILARITY_RESULTS, MAX_COMPONENT_CANDIDATES)
}

fn all_eligible_candidate_indices(metrics: &[SimilarityMetrics]) -> HashSet<usize> {
    metrics
        .iter()
        .filter(|metric| metric.is_exact || metric.passes_color_gate)
        .map(|metric| metric.index)
        .collect()
}

fn add_top_metric_candidates<F>(
    metrics: &[SimilarityMetrics],
    limit: usize,
    candidates: &mut HashSet<usize>,
    score: F,
) -> usize
where
    F: Fn(&SimilarityMetrics) -> Option<f32>,
{
    let mut ranked: Vec<(usize, f32)> = metrics
        .iter()
        .filter(|metric| !metric.is_exact && metric.passes_color_gate)
        .filter_map(|metric| score(metric).map(|value| (metric.index, value)))
        .collect();
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, |a, b| b.1.total_cmp(&a.1));
        ranked.truncate(limit);
    }
    let selected = ranked.len();
    candidates.extend(ranked.into_iter().map(|(index, _)| index));
    selected
}

fn choose_candidate_indices(
    metrics: &[SimilarityMetrics],
    settings: SimilaritySettings,
    clip_available: bool,
) -> HashSet<usize> {
    if metrics.len() <= CANDIDATE_PIPELINE_MIN_RECORDS {
        return all_eligible_candidate_indices(metrics);
    }

    let limit = component_candidate_limit(metrics.len());
    let mut candidates: HashSet<usize> = metrics
        .iter()
        .filter(|metric| metric.is_exact)
        .map(|metric| metric.index)
        .collect();
    let mut available_component = false;

    if settings.color_distribution_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.histogram_similarity
            }) > 0;
    }
    if settings.texture_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.hash_similarity
            }) > 0;
    }
    if settings.clip_weight > 0.0 && clip_available {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                metric.clip_similarity
            }) > 0;
    }
    if settings.dominant_color_weight > 0.0 {
        available_component |=
            add_top_metric_candidates(metrics, limit, &mut candidates, |metric| {
                Some(metric.dominant_similarity)
            }) > 0;
    }

    let eligible_count = metrics
        .iter()
        .filter(|metric| metric.is_exact || metric.passes_color_gate)
        .count();
    let minimum_useful = MAX_SIMILARITY_RESULTS.min(eligible_count);

    // Preserve the old full-scan semantics for zero-weight searches or sparse
    // descriptor sets that could not provide enough real candidates.
    if !available_component || candidates.len() < minimum_useful {
        return all_eligible_candidate_indices(metrics);
    }

    candidates
}

fn similarity_search(
    db_path: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageSummary>> {
    let indexing_settings = indexing_settings.sanitized();
    let conn = db::open(db_path)?;

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading texture/color index: {} image{}…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, indexing_settings.decode_workers, tx)?;
    }

    let query_image = decode_image(query_path)?;
    let (query_dominant, query_hash, query_histogram, query_material_texture) =
        visual_descriptor(&query_image);

    let query_embedding = if settings.clip_weight > 0.0 {
        match query_clip_embedding(
            embedding_service,
            query_path,
            indexing_settings.clip_threads,
            indexing_settings.clip_execution_provider,
        ) {
            Ok((embedding, model_reloaded, active_provider, fallback_reason)) => {
                let mut status = if model_reloaded {
                    format!(
                        "CLIP model initialized on {} for this query; future searches will reuse it",
                        active_provider.label()
                    )
                } else {
                    format!(
                        "Reusing loaded CLIP model on {} for query",
                        active_provider.label()
                    )
                };
                if let Some(reason) = fallback_reason {
                    status.push_str(&format!(" — {reason}"));
                }
                let _ = tx.send(WorkerMessage::Status(status));
                Some(embedding)
            }
            Err(err) => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "CLIP unavailable; using texture/color similarity only ({err})"
                )));
                None
            }
        }
    } else {
        None
    };

    let query_key = normalized_path_key(query_path);
    let mut records = db::load_search_images(db_path)?;
    let compute_hash = settings.texture_weight > 0.0;
    let compute_histogram =
        settings.color_distribution_weight > 0.0 || settings.strict_color_rejection;
    let compute_clip = settings.clip_weight > 0.0 && query_embedding.is_some();
    let large_ann_search = compute_clip && records.len() > CANDIDATE_PIPELINE_MIN_RECORDS;
    let ann_scores = if large_ann_search {
        let limit = component_candidate_limit(records.len());
        match ann::search_candidates(db_path, query_embedding.as_deref().unwrap_or(&[]), limit) {
            Ok(scores) if !scores.is_empty() => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "HNSW semantic retrieval: {} approximate CLIP candidates from {} indexed records",
                    scores.len(), records.len()
                )));
                Some(scores)
            }
            Ok(_) => None,
            Err(err) => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "HNSW unavailable; falling back to brute-force CLIP candidates ({err:#})"
                )));
                None
            }
        }
    } else {
        None
    };

    let all_rowids: HashSet<usize> = if compute_clip && ann_scores.is_none() {
        records.iter().map(|record| record.rowid).collect()
    } else {
        HashSet::new()
    };
    let fallback_embeddings = if all_rowids.is_empty() {
        HashMap::new()
    } else {
        db::load_embeddings_for_rowids(db_path, &all_rowids)?
    };
    let mut metrics = Vec::<SimilarityMetrics>::with_capacity(records.len());

    for (index, record) in records.iter().enumerate() {
        let is_exact = normalized_path_key(&record.path) == query_key;
        if is_exact {
            metrics.push(SimilarityMetrics {
                index,
                is_exact: true,
                hash_similarity: Some(1.0),
                histogram_similarity: Some(1.0),
                clip_similarity: Some(1.0),
                dominant_similarity: 1.0,
                passes_color_gate: true,
            });
            continue;
        }

        let hash_similarity = if compute_hash {
            let dhash = record
                .visual_hash
                .map(|hash| perceptual_hash_similarity(query_hash, hash));
            let material = record.material_texture.as_deref().and_then(|descriptor| {
                material_texture::similarity(&query_material_texture, descriptor)
            });
            material_texture::combine_with_dhash(dhash, material)
        } else {
            None
        };
        let histogram_similarity = if compute_histogram {
            record
                .color_histogram
                .as_deref()
                .map(|histogram| histogram_intersection(&query_histogram, histogram))
        } else {
            None
        };
        let dominant_similarity = rgb_similarity(query_dominant, record.dominant);
        let passes_gate = passes_color_gate(histogram_similarity, dominant_similarity, settings);

        // For large indexes an HNSW lookup supplies the initial semantic pool.
        // Small indexes and ANN failures preserve the exact brute-force path.
        let clip_similarity = if passes_gate && compute_clip {
            if let Some(scores) = &ann_scores {
                scores.get(&record.rowid).copied()
            } else {
                query_embedding.as_ref().and_then(|query| {
                    fallback_embeddings
                        .get(&record.rowid)
                        .map(|(embedding, normalized)| {
                            clip_similarity_with_normalized_query(query, embedding, *normalized)
                                .clamp(0.0, 1.0)
                        })
                })
            }
        } else {
            None
        };

        metrics.push(SimilarityMetrics {
            index,
            is_exact: false,
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            passes_color_gate: passes_gate,
        });
    }

    let candidate_indices = choose_candidate_indices(&metrics, settings, query_embedding.is_some());

    // ANN provides approximate semantic candidate generation only. The final
    // hybrid rerank always uses exact CLIP cosine values for every union member,
    // including candidates introduced by color/texture components.
    if compute_clip && ann_scores.is_some() {
        let candidate_rowids: HashSet<usize> = candidate_indices
            .iter()
            .map(|index| records[*index].rowid)
            .collect();
        let exact_embeddings = db::load_embeddings_for_rowids(db_path, &candidate_rowids)?;
        if let Some(query) = &query_embedding {
            for index in &candidate_indices {
                if metrics[*index].is_exact {
                    metrics[*index].clip_similarity = Some(1.0);
                    continue;
                }
                metrics[*index].clip_similarity =
                    exact_embeddings
                        .get(&records[*index].rowid)
                        .map(|(embedding, normalized)| {
                            clip_similarity_with_normalized_query(query, embedding, *normalized)
                                .clamp(0.0, 1.0)
                        });
            }
        }
    }

    if records.len() > CANDIDATE_PIPELINE_MIN_RECORDS {
        let limit = component_candidate_limit(records.len());
        let _ = tx.send(WorkerMessage::Status(format!(
            "Two-stage similarity: {} indexed records → {} exact hybrid rerank candidates (up to {limit} per enabled component)",
            records.len(),
            candidate_indices.len()
        )));
    }

    let mut scored = Vec::<(bool, ImageRecord)>::with_capacity(candidate_indices.len());
    for (index, mut record) in records.into_iter().enumerate() {
        let metric = metrics[index];
        if metric.is_exact {
            record.score = Some(1.0);
            scored.push((true, record));
            continue;
        }
        if !candidate_indices.contains(&index) || !metric.passes_color_gate {
            continue;
        }

        record.score = Some(hybrid_similarity(
            metric.hash_similarity,
            metric.histogram_similarity,
            metric.clip_similarity,
            metric.dominant_similarity,
            settings,
        ));
        scored.push((false, record));
    }

    if scored.len() > MAX_SIMILARITY_RESULTS {
        scored.select_nth_unstable_by(MAX_SIMILARITY_RESULTS, compare_ranked_records);
        scored.truncate(MAX_SIMILARITY_RESULTS);
    }
    scored.sort_by(compare_ranked_records);

    Ok(scored
        .into_iter()
        .map(|(_, record)| ImageSummary::from(record))
        .collect())
}

fn compare_ranked_records(a: &(bool, ImageRecord), b: &(bool, ImageRecord)) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| {
        b.1.score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&a.1.score.unwrap_or(f32::NEG_INFINITY))
    })
}

fn query_clip_embedding(
    embedding_service: &EmbeddingService,
    query_path: &Path,
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
) -> Result<(Vec<f32>, bool, ClipExecutionProvider, Option<String>)> {
    let response = embedding_service.embed_with_provider(
        vec![query_path.to_path_buf()],
        1,
        clip_threads,
        requested_provider,
    )?;
    let model_reloaded = response.model_reloaded;
    let active_provider = response.active_provider;
    let fallback_reason = response.fallback_reason.clone();
    let embedding = response
        .embeddings
        .into_iter()
        .next()
        .context("CLIP returned no query embedding")?;
    Ok((embedding, model_reloaded, active_provider, fallback_reason))
}

fn passes_color_gate(
    histogram_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> bool {
    if !settings.strict_color_rejection {
        return true;
    }

    if histogram_similarity
        .is_some_and(|similarity| similarity * 100.0 < settings.min_color_distribution_match)
    {
        return false;
    }

    let dominant_difference = (1.0 - dominant_similarity).clamp(0.0, 1.0) * 100.0;
    dominant_difference <= settings.max_dominant_color_difference
}

fn hybrid_similarity(
    hash_similarity: Option<f32>,
    histogram_similarity: Option<f32>,
    clip_similarity: Option<f32>,
    dominant_similarity: f32,
    settings: SimilaritySettings,
) -> f32 {
    // User-controlled weights are normalized over whichever descriptors are
    // available for a candidate. They do not need to sum to exactly 100%.
    let mut weighted = 0.0f32;
    let mut weight = 0.0f32;

    let dominant_weight = settings.dominant_color_weight.max(0.0);
    if dominant_weight > 0.0 {
        weighted += dominant_weight * dominant_similarity;
        weight += dominant_weight;
    }

    let histogram_weight = settings.color_distribution_weight.max(0.0);
    if let Some(value) = histogram_similarity.filter(|_| histogram_weight > 0.0) {
        weighted += histogram_weight * value;
        weight += histogram_weight;
    }

    let texture_weight = settings.texture_weight.max(0.0);
    if let Some(value) = hash_similarity.filter(|_| texture_weight > 0.0) {
        weighted += texture_weight * value;
        weight += texture_weight;
    }

    let clip_weight = settings.clip_weight.max(0.0);
    if let Some(value) = clip_similarity.filter(|_| clip_weight > 0.0) {
        weighted += clip_weight * value;
        weight += clip_weight;
    }

    if weight <= f32::EPSILON {
        0.0
    } else {
        (weighted / weight).clamp(0.0, 1.0)
    }
}

fn perceptual_hash_similarity(a: u64, b: u64) -> f32 {
    1.0 - ((a ^ b).count_ones() as f32 / 64.0)
}

fn histogram_intersection(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| x.min(y))
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

fn clip_similarity_with_normalized_query(
    query: &[f32],
    candidate: &[f32],
    candidate_normalized: bool,
) -> f32 {
    if query.len() != candidate.len() || query.is_empty() {
        return -1.0;
    }

    let dot = query
        .iter()
        .zip(candidate.iter())
        .map(|(&x, &y)| x * y)
        .sum::<f32>();
    if candidate_normalized {
        return dot;
    }

    let candidate_norm_sq = candidate.iter().map(|value| value * value).sum::<f32>();
    if candidate_norm_sq <= f32::EPSILON {
        -1.0
    } else {
        dot / candidate_norm_sq.sqrt()
    }
}

fn rgb_similarity(a: [u8; 3], b: [u8; 3]) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    let distance = (dr * dr + dg * dg + db * db).sqrt();
    (1.0 - distance / (255.0 * 3.0f32.sqrt())).clamp(0.0, 1.0)
}

fn normalized_path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('/', "\\");
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn inspect_image(
    path: &Path,
    thumbnail_cache_dir: &Path,
) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>, Vec<f32>)> {
    let image = decode_image(path)?;
    let (width, height) = image.dimensions();

    // Seed the exact same persistent cache used by the UI while the original
    // file is already decoded. Thumbnail cache failures never invalidate the
    // authoritative image index; the UI can still rebuild the preview later.
    let _ = thumbnail_cache::store_from_decoded(thumbnail_cache_dir, path, &image);

    let (dominant, visual_hash, color_histogram, material_texture) = visual_descriptor(&image);
    Ok((
        width,
        height,
        dominant,
        visual_hash,
        color_histogram,
        material_texture,
    ))
}

fn decode_image(path: &Path) -> Result<DynamicImage> {
    image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))
}

fn visual_descriptor(image: &DynamicImage) -> ([u8; 3], u64, Vec<f32>, Vec<f32>) {
    let color_thumb = image.thumbnail(128, 128).to_rgba8();

    // Dominant color uses a finer 8×8×8 quantization for display and the
    // chromatic/achromatic mismatch penalty.
    let mut dominant_bins = vec![(0u32, 0u64, 0u64, 0u64); 8 * 8 * 8];

    // The search histogram uses 4×4×4 RGB bins. 64 normalized values are
    // compact enough for large indexes while preserving the key distinction
    // between brown/beige materials and grayscale stone/cement.
    let mut color_histogram = vec![0.0f32; COLOR_HISTOGRAM_BINS];
    let mut histogram_pixels = 0u32;

    for pixel in color_thumb.pixels() {
        let rgba = pixel.channels();
        if rgba[3] < 24 {
            continue;
        }
        let r = rgba[0];
        let g = rgba[1];
        let b = rgba[2];

        let dominant_index = ((r as usize >> 5) * 64) + ((g as usize >> 5) * 8) + (b as usize >> 5);
        let dominant_bin = &mut dominant_bins[dominant_index];
        dominant_bin.0 += 1;
        dominant_bin.1 += r as u64;
        dominant_bin.2 += g as u64;
        dominant_bin.3 += b as u64;

        let histogram_index =
            ((r as usize >> 6) * 16) + ((g as usize >> 6) * 4) + (b as usize >> 6);
        color_histogram[histogram_index] += 1.0;
        histogram_pixels += 1;
    }

    if histogram_pixels > 0 {
        let denom = histogram_pixels as f32;
        for value in &mut color_histogram {
            *value /= denom;
        }
    }

    let dominant = dominant_bins
        .into_iter()
        .max_by_key(|bin| bin.0)
        .filter(|bin| bin.0 > 0)
        .map(|bin| {
            [
                (bin.1 / bin.0 as u64) as u8,
                (bin.2 / bin.0 as u64) as u8,
                (bin.3 / bin.0 as u64) as u8,
            ]
        })
        .unwrap_or([0, 0, 0]);

    // 64-bit difference hash: captures coarse edge/vein layout and is very
    // strong for exact/near-duplicate texture faces without adding a model.
    let gray = image.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut visual_hash = 0u64;
    let mut bit = 0u32;
    for y in 0..8 {
        for x in 0..8 {
            if gray.get_pixel(x, y)[0] > gray.get_pixel(x + 1, y)[0] {
                visual_hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }

    let material_texture = material_texture::descriptor(image);
    (dominant, visual_hash, color_histogram, material_texture)
}

pub fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg") | Some("png") | Some("tif") | Some("tiff")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn normalized_query_similarity_matches_legacy_candidate_fallback() {
        let query = [0.6, 0.8];
        let normalized_candidate = [0.6, 0.8];
        let legacy_candidate = [3.0, 4.0];
        let normalized = clip_similarity_with_normalized_query(&query, &normalized_candidate, true);
        let legacy = clip_similarity_with_normalized_query(&query, &legacy_candidate, false);
        assert!((normalized - 1.0).abs() < 1e-6);
        assert!((legacy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn perceptual_hash_prefers_identical_pattern() {
        let hash = 0xA55A_A55A_0FF0_0FF0u64;
        assert_eq!(perceptual_hash_similarity(hash, hash), 1.0);
        assert_eq!(perceptual_hash_similarity(hash, !hash), 0.0);
    }

    #[test]
    fn histogram_intersection_prefers_same_color_distribution() {
        let mut brown = vec![0.0; COLOR_HISTOGRAM_BINS];
        let mut gray = vec![0.0; COLOR_HISTOGRAM_BINS];
        brown[37] = 0.8;
        brown[38] = 0.2;
        gray[21] = 1.0;

        assert!((histogram_intersection(&brown, &brown) - 1.0).abs() < 1e-6);
        assert_eq!(histogram_intersection(&brown, &gray), 0.0);
    }

    #[test]
    fn chromatic_query_penalizes_achromatic_candidate() {
        let brown = [150, 82, 38];
        let similar_brown = [145, 88, 46];
        let gray = [128, 128, 128];

        let settings = SimilaritySettings::default();
        let colored_dominant = rgb_similarity(brown, similar_brown);
        let gray_dominant = rgb_similarity(brown, gray);

        assert!(passes_color_gate(Some(0.72), colored_dominant, settings));
        assert!(!passes_color_gate(Some(0.12), gray_dominant, settings));

        let colored_score = hybrid_similarity(
            Some(0.75),
            Some(0.72),
            Some(0.70),
            colored_dominant,
            settings,
        );
        let gray_score =
            hybrid_similarity(Some(0.75), Some(0.72), Some(0.70), gray_dominant, settings);

        assert!(colored_score > gray_score);
    }

    #[test]
    fn custom_weights_change_ranking_influence() {
        let mut texture_only = SimilaritySettings::default();
        texture_only.color_distribution_weight = 0.0;
        texture_only.texture_weight = 100.0;
        texture_only.clip_weight = 0.0;
        texture_only.dominant_color_weight = 0.0;
        texture_only.strict_color_rejection = false;

        let texture_score =
            hybrid_similarity(Some(0.92), Some(0.05), Some(0.10), 0.10, texture_only);
        assert!((texture_score - 0.92).abs() < 1e-6);

        let mut clip_only = texture_only;
        clip_only.texture_weight = 0.0;
        clip_only.clip_weight = 100.0;
        let clip_score = hybrid_similarity(Some(0.92), Some(0.05), Some(0.77), 0.10, clip_only);
        assert!((clip_score - 0.77).abs() < 1e-6);
    }

    #[test]
    fn strict_color_gate_rejects_weak_histogram_match() {
        let mut settings = SimilaritySettings::default();
        settings.min_color_distribution_match = 40.0;
        settings.max_dominant_color_difference = 100.0;
        assert!(!passes_color_gate(Some(0.25), 0.95, settings));
        assert!(passes_color_gate(Some(0.60), 0.95, settings));
    }

    #[test]
    fn all_zero_weights_are_safe() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 0.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        assert_eq!(
            hybrid_similarity(Some(1.0), Some(1.0), Some(1.0), 1.0, settings),
            0.0
        );
    }

    #[test]
    fn clip_cannot_outvote_bad_color_and_texture_match() {
        let settings = SimilaritySettings::default();
        let good = hybrid_similarity(Some(0.90), Some(0.88), Some(0.62), 0.90, settings);
        let semantically_close_but_wrong =
            hybrid_similarity(Some(0.35), Some(0.12), Some(0.95), 0.55, settings);

        assert!(good > semantically_close_but_wrong);
    }

    fn synthetic_metric(
        index: usize,
        hash: f32,
        histogram: f32,
        clip: f32,
        dominant: f32,
        passes_color_gate: bool,
    ) -> SimilarityMetrics {
        SimilarityMetrics {
            index,
            is_exact: false,
            hash_similarity: Some(hash),
            histogram_similarity: Some(histogram),
            clip_similarity: Some(clip),
            dominant_similarity: dominant,
            passes_color_gate,
        }
    }

    #[test]
    fn small_library_candidate_stage_preserves_bruteforce_eligibility() {
        let metrics: Vec<_> = (0..128)
            .map(|index| synthetic_metric(index, 0.5, 0.5, 0.5, 0.5, index % 5 != 0))
            .collect();
        let selected = choose_candidate_indices(&metrics, SimilaritySettings::default(), true);
        let expected: HashSet<_> = metrics
            .iter()
            .filter(|metric| metric.passes_color_gate)
            .map(|metric| metric.index)
            .collect();
        assert_eq!(selected, expected);
    }

    #[test]
    fn large_texture_only_search_selects_best_texture_candidates() {
        let mut settings = SimilaritySettings::default();
        settings.color_distribution_weight = 0.0;
        settings.texture_weight = 100.0;
        settings.clip_weight = 0.0;
        settings.dominant_color_weight = 0.0;
        settings.strict_color_rejection = false;

        let metrics: Vec<_> = (0..5_000)
            .map(|index| {
                synthetic_metric(
                    index,
                    index as f32 / 5_000.0,
                    1.0 - index as f32 / 5_000.0,
                    0.1,
                    0.1,
                    true,
                )
            })
            .collect();
        let selected = choose_candidate_indices(&metrics, settings, true);

        assert!(selected.contains(&4_999));
        assert!(!selected.contains(&0));
        assert_eq!(selected.len(), component_candidate_limit(metrics.len()));
    }

    #[test]
    fn strict_gate_excludes_even_a_top_component_candidate() {
        let mut settings = SimilaritySettings::default();
        settings.color_distribution_weight = 0.0;
        settings.texture_weight = 100.0;
        settings.clip_weight = 0.0;
        settings.dominant_color_weight = 0.0;

        let mut metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, 0.5, 0.5, 0.5, 0.5, true))
            .collect();
        metrics[4_999].hash_similarity = Some(1.0);
        metrics[4_999].passes_color_gate = false;

        let selected = choose_candidate_indices(&metrics, settings, true);
        assert!(!selected.contains(&4_999));
    }

    #[test]
    fn exact_query_is_always_in_large_candidate_union() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 100.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        let mut metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, index as f32 / 5_000.0, 0.0, 0.0, 0.0, true))
            .collect();
        metrics[0].is_exact = true;
        metrics[0].hash_similarity = Some(0.0);

        let selected = choose_candidate_indices(&metrics, settings, false);
        assert!(selected.contains(&0));
    }

    #[test]
    fn zero_weight_large_search_falls_back_to_full_eligible_scan() {
        let settings = SimilaritySettings {
            color_distribution_weight: 0.0,
            texture_weight: 0.0,
            clip_weight: 0.0,
            dominant_color_weight: 0.0,
            strict_color_rejection: false,
            min_color_distribution_match: 0.0,
            max_dominant_color_difference: 100.0,
        };
        let metrics: Vec<_> = (0..5_000)
            .map(|index| synthetic_metric(index, 0.0, 0.0, 0.0, 0.0, true))
            .collect();
        let selected = choose_candidate_indices(&metrics, settings, true);
        assert_eq!(selected.len(), metrics.len());
    }

    #[test]
    fn committed_batch_survives_later_rollback() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!(
            "windows-image-search-durability-{}-{nonce}.sqlite3",
            std::process::id()
        ));
        let root = PathBuf::from("C:/indexed");
        let first = root.join("first.jpg");
        let second = root.join("second.jpg");

        {
            let mut conn = db::open(&db_path).unwrap();
            {
                let transaction = conn.transaction().unwrap();
                db::upsert_image(
                    &transaction,
                    &first,
                    &root,
                    "first.jpg",
                    "jpg",
                    123,
                    456,
                    64,
                    64,
                    "",
                    "",
                    [120, 90, 60],
                    0x55AA_55AA_55AA_55AA,
                    &[1.0, 0.0, 0.0, 0.0],
                )
                .unwrap();
                transaction.commit().unwrap();
            }
            {
                let transaction = conn.transaction().unwrap();
                db::upsert_image(
                    &transaction,
                    &second,
                    &root,
                    "second.jpg",
                    "jpg",
                    123,
                    456,
                    64,
                    64,
                    "",
                    "",
                    [120, 90, 60],
                    0xAA55_AA55_AA55_AA55,
                    &[1.0, 0.0, 0.0, 0.0],
                )
                .unwrap();
                // Simulate an interrupted batch: dropping rolls it back.
            }
        }

        let records = db::load_images(&db_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, first);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
