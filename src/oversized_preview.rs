use crate::{portable, thumbnail_cache};
use anyhow::{bail, Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GrayImage, RgbImage};
use jpeg_decoder::{Decoder, PixelFormat};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

pub const PREVIEW_EDGE: u32 = 2048;
pub const PREVIEW_REVISION: i64 = 1;
pub const CACHE_DIR_NAME: &str = "oversized-previews";
const JPEG_QUALITY: u8 = 88;
const MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct PreviewAsset {
    pub path: PathBuf,
    pub image: DynamicImage,
    pub source_width: u32,
    pub source_height: u32,
    pub reused: bool,
}

pub fn cache_dir(root: &Path) -> PathBuf {
    portable::index_dir(root).join(CACHE_DIR_NAME)
}

pub fn load_current_for_root(root: &Path, source: &Path) -> Result<PreviewAsset> {
    let meta = std::fs::metadata(source)
        .with_context(|| format!("reading oversized source metadata {}", source.display()))?;
    let modified = modified_seconds(&meta);
    load_or_build(root, source, meta.len(), modified)
}

pub fn load_or_build(
    root: &Path,
    source: &Path,
    source_size: u64,
    source_modified: i64,
) -> Result<PreviewAsset> {
    ensure_jpeg(source)?;
    let (source_width, source_height) = jpeg_dimensions(source)?;
    let expected = cache_path_for_state(root, source, source_size, source_modified)?;

    if let Some(image) = load_valid_derivative(&expected) {
        seed_ui_thumbnail(root, source, &image);
        cleanup_source_dir_except(&expected)?;
        return Ok(PreviewAsset {
            path: expected,
            image,
            source_width,
            source_height,
            reused: true,
        });
    }

    let image = decode_jpeg_bounded(source)?;
    let bounded = DynamicImage::ImageRgb8(image.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE).to_rgb8());
    write_derivative(&expected, &bounded)?;
    seed_ui_thumbnail(root, source, &bounded);
    cleanup_source_dir_except(&expected)?;

    Ok(PreviewAsset {
        path: expected,
        image: bounded,
        source_width,
        source_height,
        reused: false,
    })
}

pub fn cache_path_for_state(
    root: &Path,
    source: &Path,
    source_size: u64,
    source_modified: i64,
) -> Result<PathBuf> {
    let relative = portable::relative_source_path(root, source)?;
    let source_key = source_identity_key(&relative);
    let state_key = state_key(source_size, source_modified, PREVIEW_REVISION, PREVIEW_EDGE);
    Ok(cache_dir(root).join(source_key).join(format!(
        "r{PREVIEW_REVISION}-e{PREVIEW_EDGE}-{state_key}.jpg"
    )))
}

pub fn remove_source_cache(root: &Path, source: &Path) -> Result<()> {
    let relative = portable::relative_source_path(root, source)?;
    let path = cache_dir(root).join(source_identity_key(&relative));
    if path.exists() {
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("removing oversized preview cache {}", path.display()))?;
    }
    Ok(())
}

fn ensure_jpeg(source: &Path) -> Result<()> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "jpg" && extension != "jpeg" {
        bail!(
            "bounded oversized preview is currently supported for JPEG only; refusing full decode of {}",
            source.display()
        );
    }
    Ok(())
}

fn jpeg_dimensions(source: &Path) -> Result<(u32, u32)> {
    let file = File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder
        .read_info()
        .with_context(|| format!("reading JPEG header {}", source.display()))?;
    let info = decoder
        .info()
        .context("JPEG decoder returned no image information")?;
    Ok((info.width as u32, info.height as u32))
}

fn decode_jpeg_bounded(source: &Path) -> Result<DynamicImage> {
    let file = File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder.set_max_decoding_buffer_size(MAX_DECODED_BYTES);
    let requested = PREVIEW_EDGE.min(u16::MAX as u32) as u16;
    decoder
        .scale(requested, requested)
        .with_context(|| format!("selecting bounded JPEG IDCT scale for {}", source.display()))?;
    let pixels = decoder
        .decode()
        .with_context(|| format!("bounded JPEG decode failed for {}", source.display()))?;
    let info = decoder
        .info()
        .context("JPEG decoder returned no scaled image information")?;
    let expected = info.width as usize * info.height as usize * info.pixel_format.pixel_bytes();
    if expected > MAX_DECODED_BYTES || pixels.len() > MAX_DECODED_BYTES {
        bail!(
            "bounded JPEG decode would exceed {} MiB for {}",
            MAX_DECODED_BYTES / (1024 * 1024),
            source.display()
        );
    }
    if pixels.len() != expected {
        bail!(
            "bounded JPEG decoder returned {} bytes, expected {expected}",
            pixels.len()
        );
    }

    match info.pixel_format {
        PixelFormat::RGB24 => RgbImage::from_raw(info.width as u32, info.height as u32, pixels)
            .map(DynamicImage::ImageRgb8)
            .context("invalid RGB JPEG output buffer"),
        PixelFormat::L8 => GrayImage::from_raw(info.width as u32, info.height as u32, pixels)
            .map(DynamicImage::ImageLuma8)
            .context("invalid grayscale JPEG output buffer"),
        other => bail!(
            "bounded oversized JPEG pixel format {other:?} is not supported; refusing unsafe fallback"
        ),
    }
}

fn load_valid_derivative(path: &Path) -> Option<DynamicImage> {
    if !path.is_file() {
        return None;
    }
    let image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    if image.width() > PREVIEW_EDGE || image.height() > PREVIEW_EDGE {
        let _ = std::fs::remove_file(path);
        return None;
    }
    Some(image)
}

fn write_derivative(path: &Path, image: &DynamicImage) -> Result<()> {
    let parent = path
        .parent()
        .context("oversized preview has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating oversized preview cache {}", parent.display()))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".preview-{}-{sequence}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let file = File::create(&temp)
            .with_context(|| format!("creating temporary preview {}", temp.display()))?;
        let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), JPEG_QUALITY);
        encoder
            .encode_image(image)
            .with_context(|| format!("encoding oversized preview {}", path.display()))?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&temp, path)
            .with_context(|| format!("committing oversized preview {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn seed_ui_thumbnail(root: &Path, source: &Path, image: &DynamicImage) {
    let _ = thumbnail_cache::store_from_decoded_for_root(root, source, image);
}

fn cleanup_source_dir_except(keep: &Path) -> Result<()> {
    let Some(parent) = keep.parent() else {
        return Ok(());
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn source_identity_key(relative: &Path) -> String {
    let normalized = relative
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    short_sha256(normalized.as_bytes())
}

fn state_key(size: u64, modified: i64, revision: i64, edge: u32) -> String {
    let mut bytes = Vec::with_capacity(28);
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&modified.to_le_bytes());
    bytes.extend_from_slice(&revision.to_le_bytes());
    bytes.extend_from_slice(&edge.to_le_bytes());
    short_sha256(&bytes)
}

fn short_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn modified_seconds(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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
            "wis-oversized-preview-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn cache_key_changes_with_source_state_and_revision_inputs() {
        let a = state_key(640_000_000, 100, 1, 2048);
        let b = state_key(640_000_001, 100, 1, 2048);
        let c = state_key(640_000_000, 101, 1, 2048);
        let d = state_key(640_000_000, 100, 2, 2048);
        let e = state_key(640_000_000, 100, 1, 1024);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(a, e);
    }

    #[test]
    fn valid_derivative_is_reused_and_changed_state_invalidates_old_asset() {
        let root = temp_root("reuse");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("large.jpg");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(3200, 1800, Rgb([20, 40, 60])))
            .save(&source)
            .unwrap();

        let first = load_or_build(&root, &source, 640_000_000, 100).unwrap();
        assert!(!first.reused);
        assert!(first.path.is_file());
        let second = load_or_build(&root, &source, 640_000_000, 100).unwrap();
        assert!(second.reused);
        assert_eq!(first.path, second.path);

        let third = load_or_build(&root, &source, 640_000_001, 101).unwrap();
        assert_ne!(first.path, third.path);
        assert!(!first.path.exists());
        assert!(third.path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_oversized_format_fails_without_fallback() {
        let root = temp_root("unsupported");
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("large.png");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(8, 8, Rgb([1, 2, 3])))
            .save(&source)
            .unwrap();
        let err = load_or_build(&root, &source, 640_000_000, 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("JPEG only"));
        assert!(err.contains("refusing full decode"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_cache_can_be_removed_after_source_deletion() {
        let root = temp_root("delete");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("large.jpg");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(64, 64, Rgb([1, 2, 3])))
            .save(&source)
            .unwrap();
        let asset = load_or_build(&root, &source, 640_000_000, 1).unwrap();
        assert!(asset.path.exists());
        std::fs::remove_file(&source).unwrap();
        remove_source_cache(&root, &source).unwrap();
        assert!(!asset.path.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
