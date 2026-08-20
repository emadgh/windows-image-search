use crate::portable;
use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const CACHE_EDGE: u32 = 512;
const JPEG_QUALITY: u8 = 84;
const PORTABLE_FNV_OFFSET: u64 = 0xcbf29ce484222325;
const PORTABLE_FNV_PRIME: u64 = 0x100000001b3;

pub fn cache_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("thumbnail-cache")
}

/// Legacy AppData cache identity. Keep the v0.2.9 DefaultHasher behavior so
/// migration can still locate and copy existing thumbnail entries.
pub fn cache_path(cache_dir: &Path, source: &Path) -> PathBuf {
    legacy_cache_path_with_identity(cache_dir, source, source)
}

pub fn cache_path_for_root(root: &Path, source: &Path) -> Result<PathBuf> {
    let relative = portable::relative_source_path(root, source)?;
    Ok(portable_cache_path_with_identity(
        &portable::thumbnail_dir(root),
        &relative,
        source,
    ))
}

pub fn load_cached(cache_dir: &Path, source: &Path) -> Option<DynamicImage> {
    load_cached_path(cache_path(cache_dir, source))
}

pub fn load_cached_for_root(root: &Path, source: &Path) -> Option<DynamicImage> {
    let path = cache_path_for_root(root, source).ok()?;
    load_cached_path(path)
}

pub fn valid_cache_path_for_root(root: &Path, source: &Path) -> Option<PathBuf> {
    let path = cache_path_for_root(root, source).ok()?;
    load_cached_path(path.clone()).map(|_| path)
}

pub fn store_from_decoded(
    cache_dir: &Path,
    source: &Path,
    image: &DynamicImage,
) -> Result<PathBuf> {
    let cache_path = cache_path(cache_dir, source);
    store_at_path(cache_path, image)
}

pub fn store_from_decoded_for_root(
    root: &Path,
    source: &Path,
    image: &DynamicImage,
) -> Result<PathBuf> {
    let cache_path = cache_path_for_root(root, source)?;
    store_at_path(cache_path, image)
}

pub fn load_or_build(cache_dir: &Path, source: &Path) -> Option<DynamicImage> {
    if let Some(image) = load_cached(cache_dir, source) {
        return Some(image);
    }
    build_and_store(cache_path(cache_dir, source), source)
}

pub fn load_or_build_for_root(root: &Path, source: &Path) -> Option<DynamicImage> {
    if let Some(image) = load_cached_for_root(root, source) {
        return Some(image);
    }
    let cache_path = cache_path_for_root(root, source).ok()?;
    build_and_store(cache_path, source)
}

fn legacy_cache_path_with_identity(cache_dir: &Path, identity: &Path, source: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    identity.to_string_lossy().hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(source) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(delta) = modified.duration_since(UNIX_EPOCH) {
                delta.as_secs().hash(&mut hasher);
                delta.subsec_nanos().hash(&mut hasher);
            }
        }
    }
    cache_dir.join(format!("{:016x}.jpg", hasher.finish()))
}

fn portable_cache_path_with_identity(cache_dir: &Path, identity: &Path, source: &Path) -> PathBuf {
    let (size, modified_secs, modified_nanos) = std::fs::metadata(source)
        .ok()
        .map(|meta| {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
            (
                meta.len(),
                modified.as_ref().map_or(0, |value| value.as_secs()),
                modified.map_or(0, |value| value.subsec_nanos()),
            )
        })
        .unwrap_or((0, 0, 0));
    let key = portable_key_for_state(identity, size, modified_secs, modified_nanos);
    cache_dir.join(format!("{key:016x}.jpg"))
}

fn portable_key_for_state(
    identity: &Path,
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
) -> u64 {
    let normalized = identity
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

fn load_cached_path(path: PathBuf) -> Option<DynamicImage> {
    if !path.exists() {
        return None;
    }
    let decoded = image::ImageReader::open(&path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.decode().ok());
    if decoded.is_none() {
        let _ = std::fs::remove_file(path);
    }
    decoded
}

fn store_at_path(cache_path: PathBuf, image: &DynamicImage) -> Result<PathBuf> {
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();
    write_rgb_thumbnail(&cache_path, &thumb)?;
    Ok(cache_path)
}

fn build_and_store(cache_path: PathBuf, source: &Path) -> Option<DynamicImage> {
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();
    let _ = write_rgb_thumbnail(&cache_path, &thumb);
    Some(DynamicImage::ImageRgb8(thumb))
}

fn write_rgb_thumbnail(cache_path: &Path, thumb: &image::RgbImage) -> Result<()> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating thumbnail cache {}", parent.display()))?;
    }
    let file = File::create(cache_path)
        .with_context(|| format!("creating cached thumbnail {}", cache_path.display()))?;
    let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), JPEG_QUALITY);
    encoder
        .encode_image(&DynamicImage::ImageRgb8(thumb.clone()))
        .with_context(|| format!("encoding cached thumbnail {}", cache_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("windows-image-search-{label}-{nonce}"))
    }

    #[test]
    fn cache_key_changes_when_source_size_changes() {
        let dir = temp_dir("thumb-key");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.jpg");
        std::fs::write(&source, b"a").unwrap();
        let first = cache_path(&dir, &source);
        std::fs::write(&source, b"a larger source").unwrap();
        let second = cache_path(&dir, &source);
        assert_ne!(first, second);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn portable_key_does_not_depend_on_drive_or_root_prefix() {
        let relative = Path::new("tiles/stone/face.jpg");
        let first = portable_key_for_state(relative, 12345, 55, 9);
        let second = portable_key_for_state(relative, 12345, 55, 9);
        assert_eq!(first, second);
        assert_ne!(
            first,
            portable_key_for_state(Path::new("tiles/stone/other.jpg"), 12345, 55, 9)
        );
    }

    #[test]
    fn portable_key_normalizes_windows_separator_and_ascii_case() {
        let first = portable_key_for_state(Path::new("Tiles\\Stone\\Face.JPG"), 123, 45, 6);
        let second = portable_key_for_state(Path::new("tiles/stone/face.jpg"), 123, 45, 6);
        assert_eq!(first, second);
    }

    #[test]
    fn portable_key_has_a_stable_known_value() {
        assert_eq!(
            portable_key_for_state(Path::new("tiles/stone/face.jpg"), 12345, 55, 9),
            0x0a916e50a289f87c
        );
    }

    #[test]
    fn decoded_image_can_seed_and_reload_512px_cache() {
        let dir = temp_dir("thumb-seed");
        let source_dir = dir.join("source");
        let cache_dir = dir.join("cache");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("large.png");
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(900, 700, Rgb([80, 90, 100])));
        image.save(&source).unwrap();

        let decoded = image::ImageReader::open(&source).unwrap().decode().unwrap();
        let cache_path = store_from_decoded(&cache_dir, &source, &decoded).unwrap();
        assert!(cache_path.exists());

        let cached = load_cached(&cache_dir, &source).unwrap();
        assert!(cached.width() <= CACHE_EDGE);
        assert!(cached.height() <= CACHE_EDGE);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn portable_cache_is_written_inside_root_marker() {
        let root = temp_dir("portable-thumb");
        std::fs::create_dir_all(root.join("tiles")).unwrap();
        let source = root.join("tiles").join("face.png");
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(32, 32, Rgb([1, 2, 3])));
        image.save(&source).unwrap();
        let decoded = image::ImageReader::open(&source).unwrap().decode().unwrap();
        let path = store_from_decoded_for_root(&root, &source, &decoded).unwrap();
        assert!(path.starts_with(root.join(".imagesearch").join("thumbnails")));
        assert!(load_cached_for_root(&root, &source).is_some());
        let _ = std::fs::remove_dir_all(root);
    }
}
