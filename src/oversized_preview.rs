use crate::{portable, settings, thumbnail_cache};
use anyhow::{bail, Context, Result};
use image::codecs::jpeg::JpegEncoder;
#[cfg(feature = "libvips-backend")]
use image::ImageFormat;
use image::{DynamicImage, GrayImage, ImageDecoder, RgbImage};
use jpeg_decoder::{Decoder, PixelFormat};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "libvips-backend")]
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

#[cfg(feature = "libvips-backend")]
use rs_vips::{
    voption::{Setter, VOption},
    Vips, VipsImage,
};

pub const PREVIEW_EDGE: u32 = 2048;
pub const PREVIEW_REVISION: i64 = 1;
pub const CACHE_DIR_NAME: &str = "oversized-previews";
const JPEG_QUALITY: u8 = 88;
pub const MAX_DIRECT_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const ESTIMATED_DECODE_BYTES_PER_PIXEL: u64 = 8;
const MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedBackend {
    JpegScaledDecoder,
    Libvips,
}

impl BoundedBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::JpegScaledDecoder => "scaled JPEG decoder",
            Self::Libvips => "libvips",
        }
    }
}

#[derive(Debug)]
pub struct PreviewAsset {
    pub path: PathBuf,
    pub image: DynamicImage,
    pub source_width: u32,
    pub source_height: u32,
    pub reused: bool,
    pub backend: Option<BoundedBackend>,
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
    let (source_width, source_height) = source_dimensions(source)?;
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
            backend: None,
        });
    }

    let backend = bounded_backend_for(source)?;
    let image = decode_bounded(source, backend)?;
    if image.width() > PREVIEW_EDGE || image.height() > PREVIEW_EDGE {
        bail!(
            "{} returned an image larger than the {} px preview ceiling for {}",
            backend.label(),
            PREVIEW_EDGE,
            source.display()
        );
    }
    let bounded = DynamicImage::ImageRgb8(image.to_rgb8());
    write_derivative(&expected, &bounded)?;
    seed_ui_thumbnail(root, source, &bounded);
    cleanup_source_dir_except(&expected)?;

    Ok(PreviewAsset {
        path: expected,
        image: bounded,
        source_width,
        source_height,
        reused: false,
        backend: Some(backend),
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

pub fn estimated_decode_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(ESTIMATED_DECODE_BYTES_PER_PIXEL)
}

pub fn requires_bounded_dimensions(source_size: u64, width: u32, height: u32) -> bool {
    source_size > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES
        || estimated_decode_bytes(width, height) > MAX_DIRECT_DECODE_BYTES
}

pub fn requires_bounded_decode(source: &Path, source_size: u64) -> Result<bool> {
    if source_size > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES {
        return Ok(true);
    }
    let (width, height) = source_dimensions(source)?;
    Ok(requires_bounded_dimensions(source_size, width, height))
}

pub fn bounded_backend_for(source: &Path) -> Result<BoundedBackend> {
    match source_extension(source).as_str() {
        "jpg" | "jpeg" => Ok(BoundedBackend::JpegScaledDecoder),
        "png" | "tif" | "tiff" => {
            #[cfg(feature = "libvips-backend")]
            {
                Ok(BoundedBackend::Libvips)
            }
            #[cfg(not(feature = "libvips-backend"))]
            {
                bail!(
                    "bounded preview for {} requires the libvips-backend feature; refusing unsafe full-resolution decode",
                    source.display()
                )
            }
        }
        extension => bail!(
            "bounded preview does not support .{} sources: {}",
            if extension.is_empty() {
                "<none>"
            } else {
                extension
            },
            source.display()
        ),
    }
}

fn source_extension(source: &Path) -> String {
    source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn source_dimensions(source: &Path) -> Result<(u32, u32)> {
    match source_extension(source).as_str() {
        "jpg" | "jpeg" => jpeg_dimensions(source),
        "png" | "tif" | "tiff" => image::image_dimensions(source)
            .with_context(|| format!("reading image header dimensions {}", source.display())),
        _ => bail!("unsupported oversized image format: {}", source.display()),
    }
}

fn decode_bounded(source: &Path, backend: BoundedBackend) -> Result<DynamicImage> {
    match backend {
        BoundedBackend::JpegScaledDecoder => decode_jpeg_bounded(source),
        BoundedBackend::Libvips => decode_with_libvips(source),
    }
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
    // Read only decoder metadata here. The pixel decode below still uses
    // jpeg-decoder's bounded IDCT path and never performs a full-resolution
    // image::DynamicImage decode.
    let orientation = source_orientation(source);
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

    let decoded = match info.pixel_format {
        PixelFormat::RGB24 => RgbImage::from_raw(info.width as u32, info.height as u32, pixels)
            .map(DynamicImage::ImageRgb8)
            .context("invalid RGB JPEG output buffer")?,
        PixelFormat::L8 => GrayImage::from_raw(info.width as u32, info.height as u32, pixels)
            .map(DynamicImage::ImageLuma8)
            .context("invalid grayscale JPEG output buffer")?,
        other => bail!(
            "bounded oversized JPEG pixel format {other:?} is not supported; refusing unsafe fallback"
        ),
    };

    let mut decoded = decoded;
    if let Some(orientation) = orientation {
        decoded.apply_orientation(orientation);
    }

    // jpeg-decoder::scale() selects the nearest supported IDCT scale and can
    // legitimately return one dimension above the requested edge. The buffer
    // is still bounded by MAX_DECODED_BYTES, so finish the resize on that
    // already-bounded allocation rather than falling back to a full decode.
    if decoded.width() > PREVIEW_EDGE || decoded.height() > PREVIEW_EDGE {
        Ok(decoded.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE))
    } else {
        Ok(decoded)
    }
}

fn source_orientation(source: &Path) -> Option<image::metadata::Orientation> {
    let reader = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    decoder.orientation().ok()
}

#[cfg(feature = "libvips-backend")]
fn decode_with_libvips(source: &Path) -> Result<DynamicImage> {
    ensure_libvips()?;
    let filename = source
        .to_str()
        .with_context(|| format!("libvips requires a UTF-8 source path: {}", source.display()))?;
    let edge = i32::try_from(PREVIEW_EDGE).context("preview edge exceeds libvips integer range")?;
    let thumbnail =
        VipsImage::thumbnail_with_opts(filename, edge, VOption::new().set("height", edge))
            .with_context(|| {
                format!("libvips bounded thumbnail failed for {}", source.display())
            })?;
    let width = thumbnail.get_width();
    let height = thumbnail.get_height();
    if width <= 0 || height <= 0 {
        bail!("libvips returned invalid thumbnail dimensions {width}x{height}");
    }
    if width > edge || height > edge {
        bail!(
            "libvips returned {width}x{height}, above the {PREVIEW_EDGE}px bounded-preview ceiling"
        );
    }
    let encoded = thumbnail
        .jpegsave_buffer_with_opts(VOption::new().set("Q", JPEG_QUALITY as i32))
        .with_context(|| format!("encoding libvips thumbnail for {}", source.display()))?;
    image::load_from_memory_with_format(&encoded, ImageFormat::Jpeg)
        .with_context(|| format!("decoding bounded libvips output for {}", source.display()))
}

#[cfg(feature = "libvips-backend")]
fn ensure_libvips() -> Result<()> {
    static INITIALIZED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    let result = INITIALIZED.get_or_init(|| {
        Vips::init("windows-image-search")
            .map(|_| {
                Vips::concurrency_set(2);
            })
            .map_err(|err| err.to_string())
    });
    match result {
        Ok(()) => Ok(()),
        Err(error) => bail!("libvips initialization failed: {error}"),
    }
}

#[cfg(not(feature = "libvips-backend"))]
fn decode_with_libvips(source: &Path) -> Result<DynamicImage> {
    bail!(
        "libvips backend is unavailable for {}; rebuild with --features libvips-backend",
        source.display()
    )
}

fn load_valid_derivative(path: &Path) -> Option<DynamicImage> {
    if !path.is_file() {
        return None;
    }
    let image = image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.decode().ok());
    match image {
        Some(image) if image.width() <= PREVIEW_EDGE && image.height() <= PREVIEW_EDGE => {
            Some(image)
        }
        _ => {
            let _ = std::fs::remove_file(path);
            None
        }
    }
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
        match std::fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            Err(error) if path.is_file() => {
                // Another worker may have committed the same immutable cache
                // key first. Keep that completed file and discard our temp.
                let _ = std::fs::remove_file(&temp);
                Ok(())
            }
            Err(error) => Err(error)
                .with_context(|| format!("committing oversized preview {}", path.display())),
        }
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
    use image::{ImageBuffer, ImageEncoder, Rgb};
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
        assert_eq!(first.backend, Some(BoundedBackend::JpegScaledDecoder));
        assert!(first.path.is_file());
        let second = load_or_build(&root, &source, 640_000_000, 100).unwrap();
        assert!(second.reused);
        assert_eq!(second.backend, None);
        assert_eq!(first.path, second.path);

        let third = load_or_build(&root, &source, 640_000_001, 101).unwrap();
        assert_ne!(first.path, third.path);
        assert!(!first.path.exists());
        assert!(third.path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn decoded_memory_ceiling_routes_highly_compressed_sources_to_bounded_decode() {
        assert!(!requires_bounded_dimensions(8 * 1024 * 1024, 3_000, 2_000));
        assert!(requires_bounded_dimensions(8 * 1024 * 1024, 10_000, 10_000));
        assert!(requires_bounded_dimensions(
            settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES + 1,
            100,
            100
        ));
        assert_eq!(estimated_decode_bytes(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn jpeg_keeps_existing_scaled_decoder_backend() {
        assert_eq!(
            bounded_backend_for(Path::new("large.JPEG")).unwrap(),
            BoundedBackend::JpegScaledDecoder
        );
    }

    #[test]
    fn bounded_jpeg_applies_exif_orientation_without_full_decode() {
        let root = temp_root("jpeg-orientation");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("rotated.jpg");
        let file = File::create(&source).unwrap();
        let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), 90);
        // Little-endian TIFF/EXIF IFD containing Orientation=6 (Rotate90).
        let exif = vec![
            0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        encoder.set_exif_metadata(exif).unwrap();
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(120, 60, Rgb([10, 20, 30])));
        encoder.encode_image(&image).unwrap();
        drop(encoder);

        let decoded = decode_jpeg_bounded(&source).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (60, 120));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "libvips-backend")]
    #[test]
    fn corrupt_png_returns_error_without_committing_derivative() {
        let root = temp_root("corrupt-png");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("broken.png");
        // A PNG signature plus a partial IHDR is enough to exercise the failure
        // path without ever creating a valid full-resolution pixel buffer.
        std::fs::write(
            &source,
            [
                0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
                b'D', b'R', 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00,
            ],
        )
        .unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let result = load_or_build(&root, &source, meta.len(), modified_seconds(&meta));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(not(feature = "libvips-backend"))]
    #[test]
    fn png_and_tiff_fail_closed_without_libvips_feature() {
        for source in ["large.png", "large.tif", "large.tiff"] {
            let err = bounded_backend_for(Path::new(source))
                .unwrap_err()
                .to_string();
            assert!(err.contains("libvips-backend"));
            assert!(err.contains("refusing unsafe full-resolution decode"));
        }
    }

    #[cfg(feature = "libvips-backend")]
    #[test]
    fn png_and_tiff_select_libvips_backend() {
        for source in ["large.png", "large.tif", "large.tiff"] {
            assert_eq!(
                bounded_backend_for(Path::new(source)).unwrap(),
                BoundedBackend::Libvips
            );
        }
    }

    #[cfg(feature = "libvips-backend")]
    #[test]
    fn libvips_builds_bounded_png_and_tiff_previews_and_thumbnail_cache() {
        for (label, extension) in [("png", "png"), ("tiff", "tiff")] {
            let root = temp_root(&format!("libvips-{label}"));
            std::fs::create_dir_all(root.join("images")).unwrap();
            let source = root.join("images").join(format!("large.{extension}"));
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(3000, 1500, Rgb([20, 40, 60])))
                .save(&source)
                .unwrap();
            let meta = std::fs::metadata(&source).unwrap();
            let asset = load_or_build(&root, &source, meta.len(), modified_seconds(&meta)).unwrap();
            assert_eq!(asset.backend, Some(BoundedBackend::Libvips));
            assert!(asset.image.width() <= PREVIEW_EDGE);
            assert!(asset.image.height() <= PREVIEW_EDGE);
            assert!(asset.path.is_file());
            assert!(thumbnail_cache::load_cached_for_root(&root, &source).is_some());
            let _ = std::fs::remove_dir_all(root);
        }
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
