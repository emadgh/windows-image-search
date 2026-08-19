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

pub fn cache_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("thumbnail-cache")
}

pub fn cache_path(cache_dir: &Path, source: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.to_string_lossy().hash(&mut hasher);
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

pub fn load_cached(cache_dir: &Path, source: &Path) -> Option<DynamicImage> {
    let path = cache_path(cache_dir, source);
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

pub fn store_from_decoded(
    cache_dir: &Path,
    source: &Path,
    image: &DynamicImage,
) -> Result<PathBuf> {
    let cache_path = cache_path(cache_dir, source);
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();
    write_rgb_thumbnail(&cache_path, &thumb)?;
    Ok(cache_path)
}

pub fn load_or_build(cache_dir: &Path, source: &Path) -> Option<DynamicImage> {
    if let Some(image) = load_cached(cache_dir, source) {
        return Some(image);
    }

    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();
    let cache_path = cache_path(cache_dir, source);
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
}
