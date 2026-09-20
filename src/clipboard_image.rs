use arboard::{Clipboard, Error as ClipboardError, ImageData};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CLIPBOARD_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

pub struct ClipboardQueryImage {
    pub path: PathBuf,
    pub temporary: bool,
}

/// Read either an image file or bitmap from the system clipboard.
///
/// `Ok(None)` deliberately means "let normal paste handling continue".
pub fn read_for_search() -> Result<Option<ClipboardQueryImage>, String> {
    let mut clipboard =
        Clipboard::new().map_err(|error| format!("Cannot access the clipboard: {error}"))?;

    match clipboard.get().file_list() {
        Ok(paths) => {
            if let Some(path) = paths
                .into_iter()
                .find(|path| path.is_file() && crate::indexer::is_supported_image(path))
            {
                return Ok(Some(ClipboardQueryImage {
                    path,
                    temporary: false,
                }));
            }
        }
        Err(ClipboardError::ContentNotAvailable) => {}
        Err(error) => return Err(format!("Cannot read files from the clipboard: {error}")),
    }

    let image = match clipboard.get().image() {
        Ok(image) => image,
        Err(ClipboardError::ContentNotAvailable) => return Ok(None),
        Err(error) => return Err(format!("Cannot read the clipboard image: {error}")),
    };
    let path = clipboard_temp_path()?;
    write_png(&path, image)?;
    Ok(Some(ClipboardQueryImage {
        path,
        temporary: true,
    }))
}

fn clipboard_temp_path() -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join("windows-image-search");
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "Cannot create clipboard image folder {}: {error}",
            directory.display()
        )
    })?;
    let id = NEXT_CLIPBOARD_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
    Ok(directory.join(format!("clipboard-query-{}-{id}.png", std::process::id())))
}

fn write_png(path: &Path, image: ImageData<'_>) -> Result<(), String> {
    let width =
        u32::try_from(image.width).map_err(|_| "Clipboard image width is too large".to_owned())?;
    let height = u32::try_from(image.height)
        .map_err(|_| "Clipboard image height is too large".to_owned())?;
    let expected_len = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Clipboard image dimensions are too large".to_owned())?;
    if width == 0 || height == 0 || image.bytes.len() != expected_len {
        return Err("Clipboard image has invalid dimensions or pixel data".to_owned());
    }

    image::save_buffer_with_format(
        path,
        image.bytes.as_ref(),
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| format!("Cannot save the clipboard image for search: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn clipboard_pixels_are_written_as_a_searchable_png() {
        let path = std::env::temp_dir().join(format!(
            "windows-image-search-clipboard-test-{}.png",
            std::process::id()
        ));
        let image = ImageData {
            width: 2,
            height: 1,
            bytes: Cow::Borrowed(&[255, 0, 0, 255, 0, 255, 0, 128]),
        };

        write_png(&path, image).unwrap();
        let decoded = image::open(&path).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.as_raw(), &[255, 0, 0, 255, 0, 255, 0, 128]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_clipboard_pixels_are_rejected() {
        let image = ImageData {
            width: 2,
            height: 2,
            bytes: Cow::Borrowed(&[0; 4]),
        };
        let path = std::env::temp_dir().join("invalid-clipboard-image.png");
        assert!(write_png(&path, image).is_err());
    }
}
