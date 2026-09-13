from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


preview_path = Path("src/oversized_preview.rs")
preview = preview_path.read_text(encoding="utf-8")
preview = replace_once(
    preview,
    'use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};',
    'use image::{DynamicImage, GrayImage, RgbImage};\n#[cfg(feature = "libvips-backend")]\nuse image::ImageFormat;',
    "cfg-gate ImageFormat import",
)
old_decode = '''    match info.pixel_format {
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
}'''
new_decode = '''    let decoded = match info.pixel_format {
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

    // jpeg-decoder::scale() selects the nearest supported IDCT scale and can
    // legitimately return one dimension above the requested edge. The buffer
    // is still bounded by MAX_DECODED_BYTES, so finish the resize on that
    // already-bounded allocation rather than falling back to a full decode.
    if decoded.width() > PREVIEW_EDGE || decoded.height() > PREVIEW_EDGE {
        Ok(decoded.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE))
    } else {
        Ok(decoded)
    }
}'''
preview = replace_once(preview, old_decode, new_decode, "finish bounded JPEG resize")
old_load = '''fn load_valid_derivative(path: &Path) -> Option<DynamicImage> {
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
}'''
new_load = '''fn load_valid_derivative(path: &Path) -> Option<DynamicImage> {
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
}'''
preview = replace_once(preview, old_load, new_load, "remove invalid derivative before rewrite")
old_write = '''        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&temp, path)
            .with_context(|| format!("committing oversized preview {}", path.display()))?;
        Ok(())'''
new_write = '''        match std::fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            Err(error) if path.is_file() => {
                // Another worker may have committed the same immutable cache
                // key first. Keep that completed file and discard our temp.
                let _ = std::fs::remove_file(&temp);
                Ok(())
            }
            Err(error) => Err(error)
                .with_context(|| format!("committing oversized preview {}", path.display())),
        }'''
preview = replace_once(preview, old_write, new_write, "atomic derivative commit")
old_test = '''    #[cfg(feature = "libvips-backend")]
    #[test]
    fn libvips_builds_bounded_png_preview_and_thumbnail_cache() {
        let root = temp_root("libvips-png");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("large.png");
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
    }'''
new_test = '''    #[cfg(feature = "libvips-backend")]
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
            let asset =
                load_or_build(&root, &source, meta.len(), modified_seconds(&meta)).unwrap();
            assert_eq!(asset.backend, Some(BoundedBackend::Libvips));
            assert!(asset.image.width() <= PREVIEW_EDGE);
            assert!(asset.image.height() <= PREVIEW_EDGE);
            assert!(asset.path.is_file());
            assert!(thumbnail_cache::load_cached_for_root(&root, &source).is_some());
            let _ = std::fs::remove_dir_all(root);
        }
    }'''
preview = replace_once(preview, old_test, new_test, "add TIFF libvips fixture")
preview_path.write_text(preview, encoding="utf-8")

thumb_path = Path("src/thumbnail_cache.rs")
thumb = thumb_path.read_text(encoding="utf-8")
thumb = replace_once(
    thumb,
    '''fn store_at_path(cache_path: PathBuf, image: &DynamicImage) -> Result<PathBuf> {
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();''',
    '''fn store_at_path(cache_path: PathBuf, image: &DynamicImage) -> Result<PathBuf> {
    if load_cached_path(cache_path.clone()).is_some() {
        return Ok(cache_path);
    }
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();''',
    "reuse valid thumbnail cache",
)
old_thumb_write = '''        if cache_path.exists() {
            std::fs::remove_file(cache_path)
                .with_context(|| format!("replacing cached thumbnail {}", cache_path.display()))?;
        }
        std::fs::rename(&temp, cache_path)
            .with_context(|| format!("committing cached thumbnail {}", cache_path.display()))?;
        Ok(())'''
new_thumb_write = '''        match std::fs::rename(&temp, cache_path) {
            Ok(()) => Ok(()),
            Err(error) if cache_path.is_file() => {
                let _ = std::fs::remove_file(&temp);
                Ok(())
            }
            Err(error) => Err(error)
                .with_context(|| format!("committing cached thumbnail {}", cache_path.display())),
        }'''
thumb = replace_once(thumb, old_thumb_write, new_thumb_write, "atomic thumbnail commit")
thumb_path.write_text(thumb, encoding="utf-8")

ui_path = Path("src/ui/image_preview.rs")
ui = ui_path.read_text(encoding="utf-8")
ui = ui.replace(
    "use crate::{face_detection, oversized_preview, portable, settings, thumbnail_cache};",
    "use crate::{face_detection, oversized_preview, portable, thumbnail_cache};",
)
ui_path.write_text(ui, encoding="utf-8")
