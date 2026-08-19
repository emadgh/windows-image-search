use crate::db::{self, ImageRecord};
use crate::metadata;
use anyhow::{Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use image::{imageops::FilterType, DynamicImage, GenericImageView, Pixel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const COLOR_HISTOGRAM_BINS: usize = 64;

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

    let _ = tx.send(WorkerMessage::Status(
        "Scanning folders recursively…".to_owned(),
    ));
    let mut traversal_errors = 0usize;
    for root in roots {
        if !root.exists() {
            traversal_errors += 1;
            let _ = tx.send(WorkerMessage::Error(format!(
                "Indexed root does not exist: {}",
                root.display()
            )));
            continue;
        }

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
                Ok((width, height, dominant, visual_hash, color_histogram)) => {
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
                        visual_hash,
                        &color_histogram,
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

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading visual index: {} image{} need texture/color descriptors…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, tx)?;
    }

    let _ = tx.send(WorkerMessage::Status(format!(
        "Base index updated: {changed} changed, {removed} removed. Preparing CLIP embeddings…"
    )));

    let missing = db::paths_missing_embedding(&conn)?;
    if !missing.is_empty() {
        if let Err(err) = build_embeddings(&conn, model_cache, &missing, tx) {
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
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    for (index, path) in paths.iter().enumerate() {
        match decode_image(path).map(|image| visual_descriptor(&image)) {
            Ok((_, visual_hash, color_histogram)) => {
                db::set_visual_descriptor(conn, path, visual_hash, &color_histogram)?;
            }
            Err(err) => {
                let _ = tx.send(WorkerMessage::Error(format!(
                    "Cannot build visual descriptor for {}: {err:#}",
                    path.display()
                )));
            }
        }

        if index % 25 == 0 || index + 1 == total {
            let _ = tx.send(WorkerMessage::Status(format!(
                "Building texture/color index: {}/{}",
                index + 1,
                total
            )));
        }
    }
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
            "Building CLIP index: {done}/{total}"
        )));
    }
    Ok(())
}

pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let _ = tx.send(WorkerMessage::Status(
            "Preparing hybrid visual search…".to_owned(),
        ));
        match similarity_search(&db_path, &model_cache, &query_path, settings, &tx) {
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

fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {
    let conn = db::open(db_path)?;

    // Upgrade existing v0.1.0 indexes on demand. This means a user can install
    // the fixed build and search immediately; Rescan is still recommended but
    // deleting/recreating the index is not required.
    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading texture/color index: {} image{}…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, tx)?;
    }

    let query_image = decode_image(query_path)?;
    let (query_dominant, query_hash, query_histogram) = visual_descriptor(&query_image);

    let query_embedding = match query_clip_embedding(model_cache, query_path) {
        Ok(embedding) => Some(embedding),
        Err(err) => {
            let _ = tx.send(WorkerMessage::Status(format!(
                "CLIP unavailable; using texture/color similarity only ({err})"
            )));
            None
        }
    };

    let query_key = normalized_path_key(query_path);
    let mut records = db::load_images(db_path)?;

    for record in &mut records {
        if normalized_path_key(&record.path) == query_key {
            // An exact indexed query file is useful evidence, not noise. v0.1.0
            // intentionally removed it; keeping it at 100% also gives users a
            // sanity check that the visual index is behaving correctly.
            record.score = Some(1.0);
            continue;
        }

        let hash_similarity = record
            .visual_hash
            .map(|hash| perceptual_hash_similarity(query_hash, hash));
        let histogram_similarity = record
            .color_histogram
            .as_deref()
            .map(|histogram| histogram_intersection(&query_histogram, histogram));
        let clip_similarity = query_embedding.as_ref().and_then(|query| {
            record
                .embedding
                .as_deref()
                .map(|embedding| cosine_similarity(query, embedding).clamp(0.0, 1.0))
        });
        let dominant_similarity = rgb_similarity(query_dominant, record.dominant);

        if !passes_color_gate(histogram_similarity, dominant_similarity, settings) {
            record.score = None;
            continue;
        }

        record.score = Some(hybrid_similarity(
            hash_similarity,
            histogram_similarity,
            clip_similarity,
            dominant_similarity,
            settings,
        ));
    }

    records
        .retain(|record| normalized_path_key(&record.path) == query_key || record.score.is_some());

    records.sort_by(|a, b| {
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

fn query_clip_embedding(model_cache: &Path, query_path: &Path) -> Result<Vec<f32>> {
    std::fs::create_dir_all(model_cache)?;
    let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(4);
    let mut model = ImageEmbedding::try_new(options).context("loading CLIP image model")?;
    let query_vecs = model
        .embed(vec![query_path.to_path_buf()], Some(1))
        .context("embedding query image")?;
    query_vecs
        .into_iter()
        .next()
        .context("CLIP returned no query embedding")
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

fn inspect_image(path: &Path) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>)> {
    let image = decode_image(path)?;
    let (width, height) = image.dimensions();
    let (dominant, visual_hash, color_histogram) = visual_descriptor(&image);
    Ok((width, height, dominant, visual_hash, color_histogram))
}

fn decode_image(path: &Path) -> Result<DynamicImage> {
    image::ImageReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| format!("decoding {}", path.display()))
}

fn visual_descriptor(image: &DynamicImage) -> ([u8; 3], u64, Vec<f32>) {
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

    (dominant, visual_hash, color_histogram)
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
        let brown = [150, 82, 38];
        let settings = SimilaritySettings::default();
        let good = hybrid_similarity(Some(0.90), Some(0.88), Some(0.62), 0.90, settings);
        let semantically_close_but_wrong =
            hybrid_similarity(Some(0.35), Some(0.12), Some(0.95), 0.55, settings);

        assert!(good > semantically_close_but_wrong);
    }
}
