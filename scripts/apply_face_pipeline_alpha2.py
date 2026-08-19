from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:180]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.3.0-alpha.1"', 'version = "0.3.0-alpha.2"')
replace_once(
    "src/main.rs",
    "mod face_detection;\n",
    "mod face_detection;\nmod face_pipeline;\n",
)
replace_once(
    "src/face_detection.rs",
    '''pub fn decode_oriented(path: &Path) -> Result<DynamicImage> {\n    let image = image::ImageReader::open(path)\n        .with_context(|| format!("opening image for face detection {}", path.display()))?\n        .with_guessed_format()\n        .with_context(|| {\n            format!(\n                "guessing image format for face detection {}",\n                path.display()\n            )\n        })?\n        .decode()\n        .with_context(|| format!("decoding image for face detection {}", path.display()))?;\n    Ok(apply_orientation(image, read_exif_orientation(path)))\n}\n''',
    '''pub fn decode_oriented(path: &Path) -> Result<DynamicImage> {\n    decode_oriented_with_orientation(path).map(|(image, _)| image)\n}\n\npub fn decode_oriented_with_orientation(path: &Path) -> Result<(DynamicImage, u32)> {\n    let orientation = read_exif_orientation(path);\n    let image = image::ImageReader::open(path)\n        .with_context(|| format!("opening image for face detection {}", path.display()))?\n        .with_guessed_format()\n        .with_context(|| {\n            format!(\n                "guessing image format for face detection {}",\n                path.display()\n            )\n        })?\n        .decode()\n        .with_context(|| format!("decoding image for face detection {}", path.display()))?;\n    Ok((apply_orientation(image, orientation), orientation))\n}\n''',
)
