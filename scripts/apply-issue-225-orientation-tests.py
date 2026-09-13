from pathlib import Path

path = Path("src/oversized_preview.rs")
text = path.read_text(encoding="utf-8")


def replace_once(old: str, new: str, label: str) -> None:
    global text
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    text = text.replace(old, new, 1)


replace_once(
    'use image::{DynamicImage, GrayImage, RgbImage};',
    'use image::{DynamicImage, GrayImage, ImageDecoder, RgbImage};',
    'import ImageDecoder',
)

replace_once(
'''fn decode_jpeg_bounded(source: &Path) -> Result<DynamicImage> {
    let file = File::open(source).with_context(|| format!("opening {}", source.display()))?;''',
'''fn decode_jpeg_bounded(source: &Path) -> Result<DynamicImage> {
    // Read only decoder metadata here. The pixel decode below still uses
    // jpeg-decoder's bounded IDCT path and never performs a full-resolution
    // image::DynamicImage decode.
    let orientation = source_orientation(source);
    let file = File::open(source).with_context(|| format!("opening {}", source.display()))?;''',
    'capture JPEG orientation before bounded decode',
)

replace_once(
'''    // jpeg-decoder::scale() selects the nearest supported IDCT scale and can
    // legitimately return one dimension above the requested edge. The buffer
    // is still bounded by MAX_DECODED_BYTES, so finish the resize on that
    // already-bounded allocation rather than falling back to a full decode.
    if decoded.width() > PREVIEW_EDGE || decoded.height() > PREVIEW_EDGE {
        Ok(decoded.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE))
    } else {
        Ok(decoded)
    }
}

#[cfg(feature = "libvips-backend")]''',
'''    let mut decoded = decoded;
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

#[cfg(feature = "libvips-backend")]''',
    'apply metadata orientation on bounded JPEG buffer',
)

replace_once(
'''    use image::{ImageBuffer, Rgb};''',
'''    use image::{ImageBuffer, ImageEncoder, Rgb};''',
    'import ImageEncoder in tests',
)

needle = '''    #[test]
    fn jpeg_keeps_existing_scaled_decoder_backend() {
        assert_eq!(
            bounded_backend_for(Path::new("large.JPEG")).unwrap(),
            BoundedBackend::JpegScaledDecoder
        );
    }
'''
addition = needle + '''
    #[test]
    fn bounded_jpeg_applies_exif_orientation_without_full_decode() {
        let root = temp_root("jpeg-orientation");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("rotated.jpg");
        let file = File::create(&source).unwrap();
        let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), 90);
        // Little-endian TIFF/EXIF IFD containing Orientation=6 (Rotate90).
        let exif = vec![
            0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00,
            0x01, 0x00,
            0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
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
                0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A,
                0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
                0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00,
            ],
        )
        .unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let result = load_or_build(&root, &source, meta.len(), modified_seconds(&meta));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }
'''
replace_once(needle, addition, 'add orientation and decode failure tests')

path.write_text(text, encoding="utf-8")
