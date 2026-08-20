use anyhow::{bail, Context, Result};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const DETECTION_IOU_THRESHOLD: f32 = 0.5;
const THRESHOLD_STEPS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormalizedBox {
    fn area(self) -> f32 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    fn validate(self, line: usize) -> Result<Self> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|value| !value.is_finite()) {
            bail!("line {line}: face box values must be finite");
        }
        if self.x < 0.0
            || self.y < 0.0
            || self.width <= 0.0
            || self.height <= 0.0
            || self.x + self.width > 1.000_001
            || self.y + self.height > 1.000_001
        {
            bail!("line {line}: face box must be positive normalized coordinates inside [0,1]");
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSourceMode {
    Bundled,
    External,
}

impl ModelSourceMode {
    fn parse(value: &str, line: usize) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bundled" => Ok(Self::Bundled),
            "external" => Ok(Self::External),
            other => {
                bail!("line {line}: model source mode must be bundled or external, got {other:?}")
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::External => "external",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMetadata {
    pub id: String,
    pub version: String,
    pub execution_provider: String,
    pub license: String,
    pub redistributable: bool,
    pub commercial_use: bool,
    pub source_mode: ModelSourceMode,
    pub source: String,
}

impl ModelMetadata {
    fn validate(&self, line: usize) -> Result<()> {
        for (label, value) in [
            ("model id", self.id.as_str()),
            ("model version", self.version.as_str()),
            ("execution provider", self.execution_provider.as_str()),
            ("license", self.license.as_str()),
            ("model source", self.source.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("line {line}: {label} cannot be empty");
            }
        }
        if self.source_mode == ModelSourceMode::Bundled
            && (!self.redistributable || !self.commercial_use)
        {
            bail!(
                "line {line}: restricted/non-commercial model weights cannot be marked bundled; use source_mode=external"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Prediction {
    confidence: f32,
    bbox: NormalizedBox,
}

#[derive(Clone, Debug)]
struct IdentityPair {
    query_id: String,
    query_person: String,
    candidate_id: String,
    candidate_person: String,
    similarity: f32,
}

#[derive(Clone, Debug)]
pub struct FaceBenchmarkManifest {
    pub path: PathBuf,
    pub model: ModelMetadata,
    images: BTreeSet<String>,
    ground_truth: BTreeMap<String, Vec<NormalizedBox>>,
    predictions: BTreeMap<String, Vec<Prediction>>,
    identity_pairs: Vec<IdentityPair>,
}

#[derive(Clone, Debug, Default)]
pub struct DetectionMetrics {
    pub images: usize,
    pub ground_truth_faces: usize,
    pub predictions: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub no_face_images: usize,
    pub no_face_images_with_false_positive: usize,
    pub tiny_faces: usize,
    pub tiny_face_recall: f64,
    pub small_faces: usize,
    pub small_face_recall: f64,
    pub large_faces: usize,
    pub large_face_recall: f64,
}

#[derive(Clone, Debug, Default)]
pub struct IdentityMetrics {
    pub queries: usize,
    pub pairs: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mrr: f64,
    pub same_pairs: usize,
    pub different_pairs: usize,
    pub same_distance_mean: f64,
    pub same_distance_p50: f64,
    pub same_distance_p90: f64,
    pub different_distance_mean: f64,
    pub different_distance_p10: f64,
    pub different_distance_p50: f64,
    pub best_similarity_threshold: f64,
    pub best_threshold_f1: f64,
    pub best_threshold_far: f64,
    pub best_threshold_frr: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Bucket {
    total: usize,
    matched: usize,
}

impl Bucket {
    fn recall(self) -> f64 {
        ratio(self.matched, self.total)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ThresholdResult {
    threshold: f64,
    tp: usize,
    fp: usize,
    tn: usize,
    fn_: usize,
    f1: f64,
}

pub fn validate_manifest(path: &Path) -> Result<String> {
    let manifest = load_manifest(path)?;
    Ok(format!(
        "Face benchmark manifest valid\n\nmanifest={}\nmodel={}\nmodel_version={}\nexecution_provider={}\nlicense={}\nredistributable={}\ncommercial_use={}\nsource_mode={}\nsource={}\nimages={}\nground_truth_faces={}\npredictions={}\nidentity_pairs={}\n",
        manifest.path.display(),
        manifest.model.id,
        manifest.model.version,
        manifest.model.execution_provider,
        manifest.model.license,
        manifest.model.redistributable,
        manifest.model.commercial_use,
        manifest.model.source_mode.as_str(),
        manifest.model.source,
        manifest.images.len(),
        manifest.ground_truth.values().map(Vec::len).sum::<usize>(),
        manifest.predictions.values().map(Vec::len).sum::<usize>(),
        manifest.identity_pairs.len(),
    ))
}

pub fn benchmark(path: &Path) -> Result<String> {
    let manifest = load_manifest(path)?;
    let detection = evaluate_detection(&manifest, DETECTION_IOU_THRESHOLD);
    let identity = evaluate_identity(&manifest.identity_pairs)?;
    Ok(render_report(&manifest, &detection, identity.as_ref()))
}

pub fn load_manifest(path: &Path) -> Result<FaceBenchmarkManifest> {
    if path.as_os_str().is_empty() {
        bail!("face benchmark manifest path is required");
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading face benchmark manifest {}", path.display()))?;

    let mut model: Option<ModelMetadata> = None;
    let mut images = BTreeSet::new();
    let mut ground_truth: BTreeMap<String, Vec<NormalizedBox>> = BTreeMap::new();
    let mut predictions: BTreeMap<String, Vec<Prediction>> = BTreeMap::new();
    let mut identity_pairs = Vec::new();

    for (index, raw) in content.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = raw.split('\t').map(str::trim).collect();
        let kind = columns
            .first()
            .copied()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match kind.as_str() {
            "model" => {
                expect_columns(&columns, 9, line, "model")?;
                if model.is_some() {
                    bail!("line {line}: only one model record is allowed per benchmark manifest");
                }
                let metadata = ModelMetadata {
                    id: columns[1].to_owned(),
                    version: columns[2].to_owned(),
                    execution_provider: columns[3].to_owned(),
                    license: columns[4].to_owned(),
                    redistributable: parse_bool(columns[5], line, "redistributable")?,
                    commercial_use: parse_bool(columns[6], line, "commercial_use")?,
                    source_mode: ModelSourceMode::parse(columns[7], line)?,
                    source: columns[8].to_owned(),
                };
                metadata.validate(line)?;
                model = Some(metadata);
            }
            "image" => {
                expect_columns(&columns, 2, line, "image")?;
                let image = required(columns[1], line, "image id")?;
                if !images.insert(image.to_owned()) {
                    bail!("line {line}: duplicate image declaration {image:?}");
                }
            }
            "gt" => {
                expect_columns(&columns, 6, line, "gt")?;
                let image = required(columns[1], line, "image id")?.to_owned();
                let bbox = parse_box(&columns[2..6], line)?;
                ground_truth.entry(image).or_default().push(bbox);
            }
            "pred" => {
                expect_columns(&columns, 7, line, "pred")?;
                let image = required(columns[1], line, "image id")?.to_owned();
                let confidence = parse_f32(columns[2], line, "prediction confidence")?;
                if !(0.0..=1.0).contains(&confidence) {
                    bail!("line {line}: prediction confidence must be in [0,1]");
                }
                let bbox = parse_box(&columns[3..7], line)?;
                predictions
                    .entry(image)
                    .or_default()
                    .push(Prediction { confidence, bbox });
            }
            "identity" => {
                expect_columns(&columns, 6, line, "identity")?;
                let query_id = required(columns[1], line, "query face id")?.to_owned();
                let query_person = required(columns[2], line, "query person id")?.to_owned();
                let candidate_id = required(columns[3], line, "candidate face id")?.to_owned();
                let candidate_person =
                    required(columns[4], line, "candidate person id")?.to_owned();
                if query_id == candidate_id {
                    bail!("line {line}: identity pair cannot compare a face to itself");
                }
                let similarity = parse_f32(columns[5], line, "cosine similarity")?;
                if !(-1.000_001..=1.000_001).contains(&similarity) {
                    bail!("line {line}: cosine similarity must be in [-1,1]");
                }
                identity_pairs.push(IdentityPair {
                    query_id,
                    query_person,
                    candidate_id,
                    candidate_person,
                    similarity,
                });
            }
            other => bail!("line {line}: unsupported record type {other:?}"),
        }
    }

    let model = model.context("manifest is missing required model record")?;
    for image in ground_truth.keys().chain(predictions.keys()) {
        if !images.contains(image) {
            bail!("face record references undeclared image {image:?}; add an image row first");
        }
    }
    if images.is_empty() && identity_pairs.is_empty() {
        bail!("benchmark manifest must contain detector images or identity pairs");
    }
    validate_identity_consistency(&identity_pairs)?;

    Ok(FaceBenchmarkManifest {
        path: path.to_path_buf(),
        model,
        images,
        ground_truth,
        predictions,
        identity_pairs,
    })
}

fn validate_identity_consistency(pairs: &[IdentityPair]) -> Result<()> {
    let mut query_people: HashMap<&str, &str> = HashMap::new();
    let mut seen_pairs = BTreeSet::new();
    for pair in pairs {
        if let Some(existing) = query_people.insert(&pair.query_id, &pair.query_person) {
            if existing != pair.query_person {
                bail!(
                    "identity query {:?} is assigned to multiple person ids ({:?}, {:?})",
                    pair.query_id,
                    existing,
                    pair.query_person
                );
            }
        }
        if !seen_pairs.insert((pair.query_id.as_str(), pair.candidate_id.as_str())) {
            bail!(
                "duplicate identity score for query {:?} and candidate {:?}",
                pair.query_id,
                pair.candidate_id
            );
        }
    }
    Ok(())
}

fn evaluate_detection(manifest: &FaceBenchmarkManifest, iou_threshold: f32) -> DetectionMetrics {
    let mut metrics = DetectionMetrics {
        images: manifest.images.len(),
        ..DetectionMetrics::default()
    };
    let mut tiny = Bucket::default();
    let mut small = Bucket::default();
    let mut large = Bucket::default();

    for image in &manifest.images {
        let ground_truth = manifest
            .ground_truth
            .get(image)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut predictions = manifest.predictions.get(image).cloned().unwrap_or_default();
        predictions.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then_with(|| left.bbox.x.total_cmp(&right.bbox.x))
                .then_with(|| left.bbox.y.total_cmp(&right.bbox.y))
        });

        metrics.ground_truth_faces += ground_truth.len();
        metrics.predictions += predictions.len();
        if ground_truth.is_empty() {
            metrics.no_face_images += 1;
            if !predictions.is_empty() {
                metrics.no_face_images_with_false_positive += 1;
            }
        }

        let mut matched = vec![false; ground_truth.len()];
        for prediction in predictions {
            let mut best: Option<(usize, f32)> = None;
            for (index, target) in ground_truth.iter().copied().enumerate() {
                if matched[index] {
                    continue;
                }
                let overlap = iou(prediction.bbox, target);
                if best.is_none_or(|(_, current)| overlap > current) {
                    best = Some((index, overlap));
                }
            }
            if let Some((index, overlap)) = best.filter(|(_, overlap)| *overlap >= iou_threshold) {
                matched[index] = true;
                metrics.true_positives += 1;
            } else {
                metrics.false_positives += 1;
            }
        }

        for (index, target) in ground_truth.iter().copied().enumerate() {
            let bucket = if target.area() <= 0.01 {
                &mut tiny
            } else if target.area() <= 0.04 {
                &mut small
            } else {
                &mut large
            };
            bucket.total += 1;
            if matched[index] {
                bucket.matched += 1;
            } else {
                metrics.false_negatives += 1;
            }
        }
    }

    metrics.precision = ratio(
        metrics.true_positives,
        metrics.true_positives + metrics.false_positives,
    );
    metrics.recall = ratio(
        metrics.true_positives,
        metrics.true_positives + metrics.false_negatives,
    );
    metrics.f1 = f1(metrics.precision, metrics.recall);
    metrics.tiny_faces = tiny.total;
    metrics.tiny_face_recall = tiny.recall();
    metrics.small_faces = small.total;
    metrics.small_face_recall = small.recall();
    metrics.large_faces = large.total;
    metrics.large_face_recall = large.recall();
    metrics
}

fn evaluate_identity(pairs: &[IdentityPair]) -> Result<Option<IdentityMetrics>> {
    if pairs.is_empty() {
        return Ok(None);
    }

    let mut by_query: BTreeMap<&str, Vec<&IdentityPair>> = BTreeMap::new();
    for pair in pairs {
        by_query.entry(&pair.query_id).or_default().push(pair);
    }

    let mut recall_1 = 0usize;
    let mut recall_5 = 0usize;
    let mut recall_10 = 0usize;
    let mut reciprocal_rank = 0.0f64;
    let mut same_distances = Vec::new();
    let mut different_distances = Vec::new();

    for candidates in by_query.values_mut() {
        candidates.sort_by(|left, right| {
            right
                .similarity
                .total_cmp(&left.similarity)
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        let query_person = candidates[0].query_person.as_str();
        if let Some(rank) = candidates
            .iter()
            .position(|candidate| candidate.candidate_person == query_person)
            .map(|index| index + 1)
        {
            recall_1 += usize::from(rank <= 1);
            recall_5 += usize::from(rank <= 5);
            recall_10 += usize::from(rank <= 10);
            reciprocal_rank += 1.0 / rank as f64;
        }
    }

    for pair in pairs {
        let distance = 1.0 - pair.similarity as f64;
        if pair.query_person == pair.candidate_person {
            same_distances.push(distance);
        } else {
            different_distances.push(distance);
        }
    }

    let best = best_threshold(pairs);
    let queries = by_query.len();
    let metrics = IdentityMetrics {
        queries,
        pairs: pairs.len(),
        recall_at_1: ratio(recall_1, queries),
        recall_at_5: ratio(recall_5, queries),
        recall_at_10: ratio(recall_10, queries),
        mrr: if queries == 0 {
            0.0
        } else {
            reciprocal_rank / queries as f64
        },
        same_pairs: same_distances.len(),
        different_pairs: different_distances.len(),
        same_distance_mean: mean(&same_distances),
        same_distance_p50: percentile(&same_distances, 0.50),
        same_distance_p90: percentile(&same_distances, 0.90),
        different_distance_mean: mean(&different_distances),
        different_distance_p10: percentile(&different_distances, 0.10),
        different_distance_p50: percentile(&different_distances, 0.50),
        best_similarity_threshold: best.threshold,
        best_threshold_f1: best.f1,
        best_threshold_far: ratio(best.fp, best.fp + best.tn),
        best_threshold_frr: ratio(best.fn_, best.fn_ + best.tp),
    };
    Ok(Some(metrics))
}

fn best_threshold(pairs: &[IdentityPair]) -> ThresholdResult {
    let mut best = ThresholdResult {
        threshold: 1.0,
        ..ThresholdResult::default()
    };
    for step in 0..=THRESHOLD_STEPS {
        let threshold = -1.0 + (2.0 * step as f64 / THRESHOLD_STEPS as f64);
        let mut current = ThresholdResult {
            threshold,
            ..ThresholdResult::default()
        };
        for pair in pairs {
            let same = pair.query_person == pair.candidate_person;
            let accepted = pair.similarity as f64 >= threshold;
            match (same, accepted) {
                (true, true) => current.tp += 1,
                (false, true) => current.fp += 1,
                (false, false) => current.tn += 1,
                (true, false) => current.fn_ += 1,
            }
        }
        let precision = ratio(current.tp, current.tp + current.fp);
        let recall = ratio(current.tp, current.tp + current.fn_);
        current.f1 = f1(precision, recall);
        let ordering = current.f1.total_cmp(&best.f1);
        if ordering == Ordering::Greater
            || (ordering == Ordering::Equal && current.fp < best.fp)
            || (ordering == Ordering::Equal
                && current.fp == best.fp
                && current.threshold > best.threshold)
        {
            best = current;
        }
    }
    best
}

fn render_report(
    manifest: &FaceBenchmarkManifest,
    detection: &DetectionMetrics,
    identity: Option<&IdentityMetrics>,
) -> String {
    let mut report = String::new();
    report.push_str("Face benchmark evaluation\n\n");
    report.push_str(&format!("manifest={}\n", manifest.path.display()));
    report.push_str(&format!("model={}\n", manifest.model.id));
    report.push_str(&format!("model_version={}\n", manifest.model.version));
    report.push_str(&format!(
        "execution_provider={}\n",
        manifest.model.execution_provider
    ));
    report.push_str(&format!("license={}\n", manifest.model.license));
    report.push_str(&format!(
        "redistributable={}\ncommercial_use={}\nsource_mode={}\nsource={}\n",
        manifest.model.redistributable,
        manifest.model.commercial_use,
        manifest.model.source_mode.as_str(),
        manifest.model.source
    ));
    if manifest.model.source_mode == ModelSourceMode::External {
        report.push_str("model_distribution=external/user-supplied; benchmark does not redistribute model weights\n");
    } else {
        report.push_str("model_distribution=bundled candidate; manifest license metadata permits redistribution and commercial use\n");
    }

    report.push_str("\n[detector]\n");
    report.push_str(&format!("iou_threshold={DETECTION_IOU_THRESHOLD:.2}\n"));
    report.push_str(&format!(
        "images={}\nground_truth_faces={}\npredictions={}\ntrue_positives={}\nfalse_positives={}\nfalse_negatives={}\n",
        detection.images,
        detection.ground_truth_faces,
        detection.predictions,
        detection.true_positives,
        detection.false_positives,
        detection.false_negatives
    ));
    report.push_str(&format!(
        "precision={:.6}\nrecall={:.6}\nf1={:.6}\n",
        detection.precision, detection.recall, detection.f1
    ));
    report.push_str(&format!(
        "no_face_images={}\nno_face_images_with_false_positive={}\nno_face_false_positive_rate={:.6}\n",
        detection.no_face_images,
        detection.no_face_images_with_false_positive,
        ratio(
            detection.no_face_images_with_false_positive,
            detection.no_face_images
        )
    ));
    report.push_str(&format!(
        "tiny_faces_area_le_0.01={}\ntiny_face_recall={:.6}\nsmall_faces_area_0.01_to_0.04={}\nsmall_face_recall={:.6}\nlarge_faces_area_gt_0.04={}\nlarge_face_recall={:.6}\n",
        detection.tiny_faces,
        detection.tiny_face_recall,
        detection.small_faces,
        detection.small_face_recall,
        detection.large_faces,
        detection.large_face_recall
    ));

    report.push_str("\n[identity]\n");
    if let Some(identity) = identity {
        report.push_str(&format!(
            "queries={}\npairs={}\nrecall_at_1={:.6}\nrecall_at_5={:.6}\nrecall_at_10={:.6}\nmrr={:.6}\n",
            identity.queries,
            identity.pairs,
            identity.recall_at_1,
            identity.recall_at_5,
            identity.recall_at_10,
            identity.mrr
        ));
        report.push_str(&format!(
            "same_pairs={}\nsame_cosine_distance_mean={:.6}\nsame_cosine_distance_p50={:.6}\nsame_cosine_distance_p90={:.6}\n",
            identity.same_pairs,
            identity.same_distance_mean,
            identity.same_distance_p50,
            identity.same_distance_p90
        ));
        report.push_str(&format!(
            "different_pairs={}\ndifferent_cosine_distance_mean={:.6}\ndifferent_cosine_distance_p10={:.6}\ndifferent_cosine_distance_p50={:.6}\n",
            identity.different_pairs,
            identity.different_distance_mean,
            identity.different_distance_p10,
            identity.different_distance_p50
        ));
        report.push_str(&format!(
            "best_similarity_threshold={:.4}\nbest_threshold_f1={:.6}\nbest_threshold_far={:.6}\nbest_threshold_frr={:.6}\n",
            identity.best_similarity_threshold,
            identity.best_threshold_f1,
            identity.best_threshold_far,
            identity.best_threshold_frr
        ));
    } else {
        report.push_str("status=not_evaluated; no identity rows supplied\n");
    }
    report
}

fn iou(left: NormalizedBox, right: NormalizedBox) -> f32 {
    let x1 = left.x.max(right.x);
    let y1 = left.y.max(right.y);
    let x2 = (left.x + left.width).min(right.x + right.width);
    let y2 = (left.y + left.height).min(right.y + right.height);
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    if intersection <= 0.0 {
        return 0.0;
    }
    let union = left.area() + right.area() - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn parse_box(values: &[&str], line: usize) -> Result<NormalizedBox> {
    Ok(NormalizedBox {
        x: parse_f32(values[0], line, "box x")?,
        y: parse_f32(values[1], line, "box y")?,
        width: parse_f32(values[2], line, "box width")?,
        height: parse_f32(values[3], line, "box height")?,
    }
    .validate(line)?)
}

fn parse_f32(value: &str, line: usize, label: &str) -> Result<f32> {
    let parsed = value
        .parse::<f32>()
        .with_context(|| format!("line {line}: invalid {label} {value:?}"))?;
    if !parsed.is_finite() {
        bail!("line {line}: {label} must be finite");
    }
    Ok(parsed)
}

fn parse_bool(value: &str, line: usize, label: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => bail!("line {line}: {label} must be true/false (or 1/0)"),
    }
}

fn required<'a>(value: &'a str, line: usize, label: &str) -> Result<&'a str> {
    if value.trim().is_empty() {
        bail!("line {line}: {label} cannot be empty");
    }
    Ok(value.trim())
}

fn expect_columns(columns: &[&str], expected: usize, line: usize, kind: &str) -> Result<()> {
    if columns.len() != expected {
        bail!(
            "line {line}: {kind} row requires {expected} tab-separated columns, got {}",
            columns.len()
        );
    }
    Ok(())
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn f1(precision: f64, recall: f64) -> f64 {
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn percentile(values: &[f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_manifest(label: &str, content: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "wis-face-benchmark-{label}-{}-{nonce}.tsv",
            std::process::id()
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn model() -> &'static str {
        "model\ttest-face-stack\t1\tCPU\tApache-2.0\ttrue\ttrue\tbundled\thttps://example.invalid/model.onnx\n"
    }

    #[test]
    fn detector_matching_counts_duplicate_prediction_as_false_positive() {
        let content = format!(
            "{}image\ta\nimage\tb\ngt\ta\t0.10\t0.10\t0.20\t0.20\npred\ta\t0.99\t0.10\t0.10\t0.20\t0.20\npred\ta\t0.90\t0.10\t0.10\t0.20\t0.20\npred\tb\t0.80\t0.30\t0.30\t0.20\t0.20\n",
            model()
        );
        let path = temp_manifest("detector", &content);
        let manifest = load_manifest(&path).unwrap();
        let metrics = evaluate_detection(&manifest, 0.5);
        assert_eq!(metrics.true_positives, 1);
        assert_eq!(metrics.false_positives, 2);
        assert_eq!(metrics.false_negatives, 0);
        assert_eq!(metrics.no_face_images, 1);
        assert_eq!(metrics.no_face_images_with_false_positive, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn detector_matching_handles_multiple_faces_and_size_buckets() {
        let content = format!(
            "{}image\ta\ngt\ta\t0.05\t0.05\t0.05\t0.05\ngt\ta\t0.40\t0.40\t0.30\t0.30\npred\ta\t0.95\t0.40\t0.40\t0.30\t0.30\n",
            model()
        );
        let path = temp_manifest("multi", &content);
        let metrics = evaluate_detection(&load_manifest(&path).unwrap(), 0.5);
        assert_eq!(metrics.ground_truth_faces, 2);
        assert_eq!(metrics.true_positives, 1);
        assert_eq!(metrics.false_negatives, 1);
        assert_eq!(metrics.tiny_faces, 1);
        assert_eq!(metrics.tiny_face_recall, 0.0);
        assert_eq!(metrics.large_faces, 1);
        assert_eq!(metrics.large_face_recall, 1.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn identity_ranking_and_threshold_metrics_are_deterministic() {
        let content = format!(
            "{}identity\tq1\tp1\tc1\tp2\t0.95\nidentity\tq1\tp1\tc2\tp1\t0.90\nidentity\tq1\tp1\tc3\tp3\t0.20\nidentity\tq2\tp2\tc4\tp2\t0.92\nidentity\tq2\tp2\tc5\tp1\t0.10\n",
            model()
        );
        let path = temp_manifest("identity", &content);
        let manifest = load_manifest(&path).unwrap();
        let metrics = evaluate_identity(&manifest.identity_pairs)
            .unwrap()
            .unwrap();
        assert_eq!(metrics.queries, 2);
        assert_eq!(metrics.recall_at_1, 0.5);
        assert_eq!(metrics.recall_at_5, 1.0);
        assert!((metrics.mrr - 0.75).abs() < 1e-9);
        assert!(metrics.best_threshold_f1 > 0.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restricted_model_must_be_external() {
        let content = "model\trestricted\t1\tCPU\tcustom-noncommercial\tfalse\tfalse\tbundled\thttps://example.invalid/model.onnx\nimage\ta\n";
        let path = temp_manifest("license", content);
        let error = load_manifest(&path).unwrap_err().to_string();
        assert!(error.contains("cannot be marked bundled"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restricted_external_model_is_valid_for_user_supplied_benchmarking() {
        let content = "model\trestricted\t1\tCPU\tcustom-noncommercial\tfalse\tfalse\texternal\tD:/models/restricted.onnx\nimage\ta\n";
        let path = temp_manifest("external", content);
        let manifest = load_manifest(&path).unwrap();
        assert_eq!(manifest.model.source_mode, ModelSourceMode::External);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_face_reference_is_rejected() {
        let content = format!("{}gt\tmissing\t0.1\t0.1\t0.2\t0.2\n", model());
        let path = temp_manifest("missing-image", &content);
        let error = load_manifest(&path).unwrap_err().to_string();
        assert!(error.contains("undeclared image"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn duplicate_identity_pair_is_rejected() {
        let content = format!(
            "{}identity\tq\tp1\tc\tp2\t0.2\nidentity\tq\tp1\tc\tp2\t0.3\n",
            model()
        );
        let path = temp_manifest("duplicate-pair", &content);
        let error = load_manifest(&path).unwrap_err().to_string();
        assert!(error.contains("duplicate identity score"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn iou_is_one_for_identical_boxes_and_zero_for_disjoint_boxes() {
        let a = NormalizedBox {
            x: 0.1,
            y: 0.1,
            width: 0.2,
            height: 0.2,
        };
        let b = NormalizedBox {
            x: 0.8,
            y: 0.8,
            width: 0.1,
            height: 0.1,
        };
        assert!((iou(a, a) - 1.0).abs() < 1e-6);
        assert_eq!(iou(a, b), 0.0);
    }
}
