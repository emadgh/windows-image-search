use crate::db::{self, ImageRecord};
use crate::metadata;
use anyhow::{Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use image::{GenericImageView, Pixel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Debug)]
pub enum WorkerMessage {
    Status(String),
    Progress { done: usize, total: usize },
    Reload,
    SimilarityResults(Vec<ImageRecord>),
    Error(String),
    Idle,
}

pub fn spawn_rescan(
    db_path: PathBuf,
    model_cache: PathBuf,
    roots: Vec<PathBuf>,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &model_cache, &roots, &tx);
        if let Err(err) = result {
            let _ = tx.send(WorkerMessage::Error(format!("Indexing failed: {err:#}")));
        }
        let _ = tx.send(WorkerMessage::Reload);
        let _ = tx.send(WorkerMessage::Idle);
    });
}

fn rescan(
    db_path: &Path,
    model_cache: &Path,
    roots: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let conn = db::open(db_path)?;
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();

    let _ = tx.send(WorkerMessage::Status("Scanning folders…".to_owned()));
    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_file() && is_supported_image(entry.path()) {
                candidates.push((root.clone(), entry.into_path()));
            }
        }
    }

    let total = candidates.len();
    let mut seen_by_root: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let mut changed = 0usize;

    for (index, (root, path)) in candidates.iter().enumerate() {
        seen_by_root
            .entry(root.clone())
            .or_default()
            .push(path.clone());

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

        let unchanged = db::existing_file_state(&conn, path)?
            .map(|(old_size, old_modified, _)| old_size == size && old_modified == modified)
            .unwrap_or(false);

        if !unchanged {
            match inspect_image(path) {
                Ok((width, height, dominant)) => {
                    let text = metadata::extract(path);
                    let file_name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default();
                    let extension = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    db::upsert_image(
                        &conn,
                        path,
                        root,
                        file_name,
                        &extension,
                        size,
                        modified,
                        width,
                        height,
                        &text.description,
                        &text.keywords,
                        dominant,
                    )?;
                    changed += 1;
                }
                Err(err) => {
                    let _ = tx.send(WorkerMessage::Error(format!(
                        "Cannot decode {}: {err:#}",
                        path.display()
                    )));
                }
            }
        }

        if index % 5 == 0 || index + 1 == total {
            let _ = tx.send(WorkerMessage::Progress {
                done: index + 1,
                total,
            });
        }
    }

    let mut removed = 0usize;
    for root in roots {
        let seen = seen_by_root.get(root).cloned().unwrap_or_default();
        removed += db::delete_missing_for_root(&conn, root, &seen)?;
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Base index updated: {changed} changed, {removed} removed. Preparing visual embeddings…"
    )));

    let missing = db::paths_missing_embedding(&conn)?;
    if !missing.is_empty() {
        if let Err(err) = build_embeddings(&conn, model_cache, &missing, tx) {
            let _ = tx.send(WorkerMessage::Error(format!(
                "Base index is ready, but CLIP indexing is unavailable: {err:#}"
            )));
        }
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Index ready: {total} image{}",
        if total == 1 { "" } else { "s" }
    )));
    Ok(())
}

fn build_embeddings(
    conn: &rusqlite::Connection,
    model_cache: &Path,
    paths: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    std::fs::create_dir_all(model_cache)?;
    let _ = tx.send(WorkerMessage::Status(
        "Loading CLIP model (first use may download it)…".to_owned(),
    ));
    let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(4);
    let mut model = ImageEmbedding::try_new(options).context("loading CLIP image model")?;

    let total = paths.len();
    for (batch_index, batch) in paths.chunks(16).enumerate() {
        let batch_paths = batch.to_vec();
        let embeddings = model
            .embed(batch_paths, Some(16))
            .with_context(|| format!("embedding image batch {}", batch_index + 1))?;
        for (path, embedding) in batch.iter().zip(embeddings.iter()) {
            db::set_embedding(conn, path, embedding)?;
        }
        let done = ((batch_index + 1) * 16).min(total);
        let _ = tx.send(WorkerMessage::Status(format!(
            "Building visual index: {done}/{total}"
        )));
    }
    Ok(())
}

pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let _ = tx.send(WorkerMessage::Status(
            "Loading CLIP model for similarity search…".to_owned(),
        ));
        match similarity_search(&db_path, &model_cache, &query_path) {
            Ok(results) => {
                let _ = tx.send(WorkerMessage::SimilarityResults(results));
                let _ = tx.send(WorkerMessage::Status(format!(
                    "Similarity search complete: {} matches",
                    db::load_images(&db_path).map(|v| v.len()).unwrap_or(0)
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

fn similarity_search(db_path: &Path, model_cache: &Path, query_path: &Path) -> Result<Vec<ImageRecord>> {
    let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(4);
    let mut model = ImageEmbedding::try_new(options).context("loading CLIP image model")?;
    let query_vecs = model
        .embed(vec![query_path.to_path_buf()], Some(1))
        .context("embedding query image")?;
    let query = query_vecs
        .first()
        .context("CLIP returned no query embedding")?;

    let mut records = db::load_images(db_path)?;
    records.retain(|record| record.embedding.is_some() && record.path != query_path);
    for record in &mut records {
        if let Some(embedding) = &record.embedding {
            record.score = Some(cosine_similarity(query, embedding));
        }
    }
    records.sort_by(|a, b| {
        b.score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&a.score.unwrap_or(f32::NEG_INFINITY))
    });
    Ok(records)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    let mut dot = 0.0f32;
    let mut a2 = 0.0f32;
    let mut b2 = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        a2 += x * x;
        b2 += y * y;
    }
    let denom = a2.sqrt() * b2.sqrt();
    if denom <= f32::EPSILON {
        -1.0
    } else {
        dot / denom
    }
}

fn inspect_image(path: &Path) -> Result<(u32, u32, [u8; 3])> {
    let image = image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))?;
    let (width, height) = image.dimensions();
    let thumb = image.thumbnail(96, 96).to_rgba8();

    // Quantized RGB histogram: choose the most populated color cube, then
    // average the pixels inside that cube. This is more useful than a plain
    // image-wide mean for images with large white/black backgrounds.
    let mut bins = vec![(0u32, 0u64, 0u64, 0u64); 8 * 8 * 8];
    for pixel in thumb.pixels() {
        let rgba = pixel.channels();
        if rgba[3] < 24 {
            continue;
        }
        let r = rgba[0];
        let g = rgba[1];
        let b = rgba[2];
        let idx = ((r as usize >> 5) * 64) + ((g as usize >> 5) * 8) + (b as usize >> 5);
        let bin = &mut bins[idx];
        bin.0 += 1;
        bin.1 += r as u64;
        bin.2 += g as u64;
        bin.3 += b as u64;
    }

    let dominant = bins
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

    Ok((width, height, dominant))
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
