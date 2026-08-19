use crate::db;
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DimensionBuckets {
    up_to_half_mp: usize,
    half_to_two_mp: usize,
    two_to_eight_mp: usize,
    eight_to_twenty_mp: usize,
    over_twenty_mp: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OrientationCounts {
    landscape: usize,
    portrait: usize,
    square: usize,
    invalid: usize,
}

#[derive(Clone, Debug, Default)]
struct DimensionStats {
    valid: usize,
    invalid: usize,
    width_min: u32,
    width_p50: u32,
    width_p90: u32,
    width_max: u32,
    height_min: u32,
    height_p50: u32,
    height_p90: u32,
    height_max: u32,
    megapixels_min: f64,
    megapixels_p50: f64,
    megapixels_p90: f64,
    megapixels_p95: f64,
    megapixels_max: f64,
    buckets: DimensionBuckets,
    orientations: OrientationCounts,
}

pub fn benchmark(db_path: &Path) -> Result<String> {
    let records = db::load_image_summaries(db_path)?;
    let indexed_images = records.len();
    let existing_sources = records
        .iter()
        .filter(|record| record.path.is_file())
        .count();
    let missing_sources = indexed_images.saturating_sub(existing_sources);
    let indexed_source_bytes: u64 = records.iter().map(|record| record.size).sum();
    let db_bytes = std::fs::metadata(db_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let mut extension_counts = BTreeMap::<String, usize>::new();
    for record in &records {
        let extension = if record.extension.trim().is_empty() {
            "<none>".to_owned()
        } else {
            record.extension.trim().to_ascii_lowercase()
        };
        *extension_counts.entry(extension).or_default() += 1;
    }

    let dimensions = summarize_dimensions(
        &records
            .iter()
            .map(|record| (record.width, record.height))
            .collect::<Vec<_>>(),
    );

    let mut report = String::new();
    writeln!(report, "Windows Image Search Library Profile")?;
    writeln!(report, "application_version=v{}", env!("CARGO_PKG_VERSION"))?;
    writeln!(report, "production_behavior_changed=false")?;
    writeln!(report, "indexed_images={indexed_images}")?;
    writeln!(report, "existing_source_files={existing_sources}")?;
    writeln!(report, "missing_source_files={missing_sources}")?;
    writeln!(report, "indexed_source_bytes={indexed_source_bytes}")?;
    writeln!(report, "index_database_bytes={db_bytes}")?;
    writeln!(report, "extensions_distinct={}", extension_counts.len())?;
    for (extension, count) in extension_counts {
        writeln!(report, "extension.{extension}={count}")?;
    }

    writeln!(report, "dimensions_valid={}", dimensions.valid)?;
    writeln!(report, "dimensions_invalid={}", dimensions.invalid)?;
    writeln!(report, "width_min_px={}", dimensions.width_min)?;
    writeln!(report, "width_p50_px={}", dimensions.width_p50)?;
    writeln!(report, "width_p90_px={}", dimensions.width_p90)?;
    writeln!(report, "width_max_px={}", dimensions.width_max)?;
    writeln!(report, "height_min_px={}", dimensions.height_min)?;
    writeln!(report, "height_p50_px={}", dimensions.height_p50)?;
    writeln!(report, "height_p90_px={}", dimensions.height_p90)?;
    writeln!(report, "height_max_px={}", dimensions.height_max)?;
    writeln!(report, "megapixels_min={:.3}", dimensions.megapixels_min)?;
    writeln!(report, "megapixels_p50={:.3}", dimensions.megapixels_p50)?;
    writeln!(report, "megapixels_p90={:.3}", dimensions.megapixels_p90)?;
    writeln!(report, "megapixels_p95={:.3}", dimensions.megapixels_p95)?;
    writeln!(report, "megapixels_max={:.3}", dimensions.megapixels_max)?;
    writeln!(
        report,
        "megapixel_bucket.up_to_0_5={}",
        dimensions.buckets.up_to_half_mp
    )?;
    writeln!(
        report,
        "megapixel_bucket.0_5_to_2={}",
        dimensions.buckets.half_to_two_mp
    )?;
    writeln!(
        report,
        "megapixel_bucket.2_to_8={}",
        dimensions.buckets.two_to_eight_mp
    )?;
    writeln!(
        report,
        "megapixel_bucket.8_to_20={}",
        dimensions.buckets.eight_to_twenty_mp
    )?;
    writeln!(
        report,
        "megapixel_bucket.over_20={}",
        dimensions.buckets.over_twenty_mp
    )?;
    writeln!(
        report,
        "orientation.landscape={}",
        dimensions.orientations.landscape
    )?;
    writeln!(
        report,
        "orientation.portrait={}",
        dimensions.orientations.portrait
    )?;
    writeln!(
        report,
        "orientation.square={}",
        dimensions.orientations.square
    )?;
    writeln!(
        report,
        "orientation.invalid={}",
        dimensions.orientations.invalid
    )?;
    writeln!(
        report,
        "notes=Counts and dimensions come from the current local SQLite index. Source existence is checked at benchmark time; indexed_source_bytes uses the persisted indexed file sizes so the profile does not re-read every source file."
    )?;
    Ok(report)
}

fn summarize_dimensions(dimensions: &[(u32, u32)]) -> DimensionStats {
    let mut widths = Vec::<u32>::new();
    let mut heights = Vec::<u32>::new();
    let mut megapixels = Vec::<f64>::new();
    let mut buckets = DimensionBuckets::default();
    let mut orientations = OrientationCounts::default();

    for &(width, height) in dimensions {
        if width == 0 || height == 0 {
            orientations.invalid += 1;
            continue;
        }

        widths.push(width);
        heights.push(height);
        let mp = width as f64 * height as f64 / 1_000_000.0;
        megapixels.push(mp);

        if mp <= 0.5 {
            buckets.up_to_half_mp += 1;
        } else if mp <= 2.0 {
            buckets.half_to_two_mp += 1;
        } else if mp <= 8.0 {
            buckets.two_to_eight_mp += 1;
        } else if mp <= 20.0 {
            buckets.eight_to_twenty_mp += 1;
        } else {
            buckets.over_twenty_mp += 1;
        }

        if width > height {
            orientations.landscape += 1;
        } else if height > width {
            orientations.portrait += 1;
        } else {
            orientations.square += 1;
        }
    }

    widths.sort_unstable();
    heights.sort_unstable();
    megapixels.sort_by(f64::total_cmp);

    DimensionStats {
        valid: widths.len(),
        invalid: dimensions.len().saturating_sub(widths.len()),
        width_min: widths.first().copied().unwrap_or(0),
        width_p50: percentile_u32(&widths, 0.50),
        width_p90: percentile_u32(&widths, 0.90),
        width_max: widths.last().copied().unwrap_or(0),
        height_min: heights.first().copied().unwrap_or(0),
        height_p50: percentile_u32(&heights, 0.50),
        height_p90: percentile_u32(&heights, 0.90),
        height_max: heights.last().copied().unwrap_or(0),
        megapixels_min: megapixels.first().copied().unwrap_or(0.0),
        megapixels_p50: percentile_f64(&megapixels, 0.50),
        megapixels_p90: percentile_f64(&megapixels, 0.90),
        megapixels_p95: percentile_f64(&megapixels, 0.95),
        megapixels_max: megapixels.last().copied().unwrap_or(0.0),
        buckets,
        orientations,
    }
}

fn percentile_index(len: usize, percentile: f64) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some((((len - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize).min(len - 1))
}

fn percentile_u32(sorted: &[u32], percentile: f64) -> u32 {
    percentile_index(sorted.len(), percentile)
        .map(|index| sorted[index])
        .unwrap_or(0)
}

fn percentile_f64(sorted: &[f64], percentile: f64) -> f64 {
    percentile_index(sorted.len(), percentile)
        .map(|index| sorted[index])
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_profile_counts_buckets_and_orientations() {
        let stats = summarize_dimensions(&[
            (640, 480),
            (1_000, 1_000),
            (2_000, 1_500),
            (4_000, 3_000),
            (6_000, 4_000),
            (1_000, 2_000),
            (0, 100),
        ]);

        assert_eq!(stats.valid, 6);
        assert_eq!(stats.invalid, 1);
        assert_eq!(stats.buckets.up_to_half_mp, 1);
        assert_eq!(stats.buckets.half_to_two_mp, 2);
        assert_eq!(stats.buckets.two_to_eight_mp, 1);
        assert_eq!(stats.buckets.eight_to_twenty_mp, 1);
        assert_eq!(stats.buckets.over_twenty_mp, 1);
        assert_eq!(stats.orientations.landscape, 4);
        assert_eq!(stats.orientations.portrait, 1);
        assert_eq!(stats.orientations.square, 1);
        assert_eq!(stats.orientations.invalid, 1);
    }

    #[test]
    fn percentile_index_is_stable_for_small_samples() {
        assert_eq!(percentile_index(0, 0.5), None);
        assert_eq!(percentile_index(1, 0.9), Some(0));
        assert_eq!(percentile_index(5, 0.5), Some(2));
        assert_eq!(percentile_index(5, 0.9), Some(4));
    }

    #[test]
    fn empty_dimensions_return_zero_statistics() {
        let stats = summarize_dimensions(&[]);
        assert_eq!(stats.valid, 0);
        assert_eq!(stats.invalid, 0);
        assert_eq!(stats.width_p50, 0);
        assert_eq!(stats.megapixels_p95, 0.0);
    }
}
