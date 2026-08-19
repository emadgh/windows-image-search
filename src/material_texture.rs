use image::{imageops::FilterType, DynamicImage, GrayImage};
use std::f32::consts::PI;

pub const VERSION: i64 = 1;
pub const DIM: usize = 48;
const GRADIENT_BINS: usize = 8;
const LBP_BINS: usize = 16;
const SCALES: [u32; 2] = [96, 48];

pub fn descriptor(image: &DynamicImage) -> Vec<f32> {
    let mut output = Vec::with_capacity(DIM);
    for size in SCALES {
        let gray = image.resize_exact(size, size, FilterType::Triangle).to_luma8();
        output.extend(gradient_histogram(&gray));
    }
    for size in SCALES {
        let gray = image.resize_exact(size, size, FilterType::Triangle).to_luma8();
        output.extend(lbp_histogram(&gray));
    }
    debug_assert_eq!(output.len(), DIM);
    output
}

pub fn similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != DIM || b.len() != DIM {
        return None;
    }
    let ranges = [
        0..GRADIENT_BINS,
        GRADIENT_BINS..GRADIENT_BINS * 2,
        GRADIENT_BINS * 2..GRADIENT_BINS * 2 + LBP_BINS,
        GRADIENT_BINS * 2 + LBP_BINS..DIM,
    ];
    let score = ranges
        .iter()
        .map(|range| histogram_intersection(&a[range.clone()], &b[range.clone()]))
        .sum::<f32>()
        / ranges.len() as f32;
    Some(score.clamp(0.0, 1.0))
}

pub fn combine_with_dhash(dhash: Option<f32>, material: Option<f32>) -> Option<f32> {
    match (dhash, material) {
        (Some(hash), Some(texture)) => {
            // Material statistics should dominate general texture matching,
            // while dHash keeps a strong near-duplicate/layout signal.
            let blended = 0.35 * hash + 0.65 * texture;
            Some(blended.max(0.92 * hash).clamp(0.0, 1.0))
        }
        (Some(hash), None) => Some(hash.clamp(0.0, 1.0)),
        (None, Some(texture)) => Some(texture.clamp(0.0, 1.0)),
        (None, None) => None,
    }
}

fn gradient_histogram(gray: &GrayImage) -> Vec<f32> {
    let mut hist = vec![0.0f32; GRADIENT_BINS];
    let width = gray.width();
    let height = gray.height();
    if width < 3 || height < 3 {
        return hist;
    }

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let gx = gray.get_pixel(x + 1, y)[0] as f32 - gray.get_pixel(x - 1, y)[0] as f32;
            let gy = gray.get_pixel(x, y + 1)[0] as f32 - gray.get_pixel(x, y - 1)[0] as f32;
            let magnitude = (gx * gx + gy * gy).sqrt();
            if magnitude <= f32::EPSILON {
                continue;
            }
            let mut angle = gy.atan2(gx);
            if angle < 0.0 {
                angle += PI;
            }
            if angle >= PI {
                angle -= PI;
            }
            let bin = ((angle / PI) * GRADIENT_BINS as f32).floor() as usize;
            hist[bin.min(GRADIENT_BINS - 1)] += magnitude;
        }
    }
    normalize_histogram(&mut hist);
    hist
}

fn lbp_histogram(gray: &GrayImage) -> Vec<f32> {
    let mut hist = vec![0.0f32; LBP_BINS];
    let width = gray.width();
    let height = gray.height();
    if width < 3 || height < 3 {
        return hist;
    }

    const NEIGHBORS: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
    ];

    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let center = gray.get_pixel(x, y)[0];
            let mut code = 0u8;
            for (bit, (dx, dy)) in NEIGHBORS.iter().copied().enumerate() {
                let nx = (x as i32 + dx) as u32;
                let ny = (y as i32 + dy) as u32;
                if gray.get_pixel(nx, ny)[0] >= center {
                    code |= 1u8 << bit;
                }
            }
            let invariant = rotation_invariant_code(code);
            hist[(invariant as usize) >> 4] += 1.0;
        }
    }
    normalize_histogram(&mut hist);
    hist
}

fn rotation_invariant_code(code: u8) -> u8 {
    (0..8).map(|shift| code.rotate_right(shift)).min().unwrap_or(code)
}

fn normalize_histogram(hist: &mut [f32]) {
    let sum: f32 = hist.iter().sum();
    if sum > f32::EPSILON {
        for value in hist {
            *value /= sum;
        }
    }
}

fn histogram_intersection(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&left, &right)| left.min(right))
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};

    fn striped(width: u32, height: u32, vertical: bool) -> DynamicImage {
        let mut image = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let axis = if vertical { x } else { y };
                let value = if (axis / 8) % 2 == 0 { 32 } else { 224 };
                image.put_pixel(x, y, Luma([value]));
            }
        }
        DynamicImage::ImageLuma8(image)
    }

    #[test]
    fn descriptor_has_fixed_compact_dimension_and_normalized_segments() {
        let descriptor = descriptor(&striped(128, 128, true));
        assert_eq!(descriptor.len(), DIM);
        let segments = [&descriptor[0..8], &descriptor[8..16], &descriptor[16..32], &descriptor[32..48]];
        for segment in segments {
            let sum: f32 = segment.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4 || sum == 0.0);
        }
    }

    #[test]
    fn rotation_invariant_lbp_code_ignores_bit_rotation() {
        let code = 0b1110_0001u8;
        let canonical = rotation_invariant_code(code);
        for shift in 0..8 {
            assert_eq!(canonical, rotation_invariant_code(code.rotate_left(shift)));
        }
    }

    #[test]
    fn resize_of_same_material_remains_highly_similar() {
        let original = striped(160, 160, true);
        let resized = original.resize_exact(104, 104, FilterType::Triangle);
        let a = descriptor(&original);
        let b = descriptor(&resized);
        assert!(similarity(&a, &b).unwrap() > 0.80);
    }

    #[test]
    fn perpendicular_texture_is_less_similar_than_same_texture() {
        let vertical = descriptor(&striped(128, 128, true));
        let vertical_copy = descriptor(&striped(144, 144, true));
        let horizontal = descriptor(&striped(128, 128, false));
        let same = similarity(&vertical, &vertical_copy).unwrap();
        let different = similarity(&vertical, &horizontal).unwrap();
        assert!(same > different);
    }

    #[test]
    fn dhash_still_protects_near_duplicate_signal() {
        let combined = combine_with_dhash(Some(1.0), Some(0.2)).unwrap();
        assert!(combined >= 0.92);
    }
}
