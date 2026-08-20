from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:220]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


def insert_after_once(path: str, anchor: str, addition: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if addition in text:
        return
    count = text.count(anchor)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {anchor[:220]!r}")
    p.write_text(text.replace(anchor, anchor + addition, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.3.0-alpha.1"', 'version = "0.3.0-alpha.2"')

main = Path("src/main.rs")
main_text = main.read_text(encoding="utf-8")
if "mod face_pipeline;" not in main_text:
    main_text = main_text.replace("mod face_detection;\n", "mod face_detection;\nmod face_pipeline;\n", 1)
if "mod face_scope;" not in main_text:
    main_text = main_text.replace("mod face_pipeline;\n", "mod face_pipeline;\nmod face_scope;\n", 1)
main.write_text(main_text, encoding="utf-8")

replace_once(
    "src/face_detection.rs",
    '''pub fn decode_oriented(path: &Path) -> Result<DynamicImage> {\n    let image = image::ImageReader::open(path)\n        .with_context(|| format!("opening image for face detection {}", path.display()))?\n        .with_guessed_format()\n        .with_context(|| {\n            format!(\n                "guessing image format for face detection {}",\n                path.display()\n            )\n        })?\n        .decode()\n        .with_context(|| format!("decoding image for face detection {}", path.display()))?;\n    Ok(apply_orientation(image, read_exif_orientation(path)))\n}\n''',
    '''pub fn decode_oriented(path: &Path) -> Result<DynamicImage> {\n    decode_oriented_with_orientation(path).map(|(image, _)| image)\n}\n\npub fn decode_oriented_with_orientation(path: &Path) -> Result<(DynamicImage, u32)> {\n    let orientation = read_exif_orientation(path);\n    let image = image::ImageReader::open(path)\n        .with_context(|| format!("opening image for face detection {}", path.display()))?\n        .with_guessed_format()\n        .with_context(|| {\n            format!(\n                "guessing image format for face detection {}",\n                path.display()\n            )\n        })?\n        .decode()\n        .with_context(|| format!("decoding image for face detection {}", path.display()))?;\n    Ok((apply_orientation(image, orientation), orientation))\n}\n''',
)

replace_once(
    "src/ui/collections.rs",
    "use crate::db::{self, Collection, CollectionMembership, ImageSummary};\n",
    "use crate::db::{self, Collection, CollectionMembership, ImageSummary};\nuse crate::face_scope;\n",
)
replace_once(
    "src/ui/collections.rs",
    "    effective: HashMap<i64, HashSet<PathBuf>>,\n    new_name: String,\n",
    "    effective: HashMap<i64, HashSet<PathBuf>>,\n    face_detection: HashMap<i64, bool>,\n    new_name: String,\n",
)
insert_after_once(
    "src/ui/collections.rs",
    "        self.items = db::load_collections(db_path)?;\n",
    "        self.face_detection = face_scope::load_collection_flags(db_path)?;\n",
)
replace_once(
    "src/ui/collections.rs",
    "    Rename(i64, String),\n    Delete(i64),\n",
    "    Rename(i64, String),\n    SetFaceDetection(i64, bool),\n    Delete(i64),\n",
)
insert_after_once(
    "src/ui/collections.rs",
    '                ui.small("Deleting a collection only removes its membership records; image files stay untouched.");\n',
    '''\n                let mut detect_faces = self\n                    .collections\n                    .face_detection\n                    .get(&id)\n                    .copied()\n                    .unwrap_or(false);\n                if ui\n                    .add_enabled(\n                        !self.busy,\n                        egui::Checkbox::new(\n                            &mut detect_faces,\n                            "Detect faces in this collection",\n                        ),\n                    )\n                    .on_hover_text(\n                        "Only effective members of face-enabled collections are sent to the face detector.",\n                    )\n                    .changed()\n                {\n                    action = Some(CollectionAction::SetFaceDetection(id, detect_faces));\n                }\n                ui.small(\n                    "Off by default. Texture-only collections are skipped completely by face detection. Existing face data is kept when this is turned off.",\n                );\n''',
)
insert_after_once(
    "src/ui/collections.rs",
    '''                CollectionAction::Rename(id, name) => {\n                    db::rename_collection(&self.db_path, id, &name)?;\n                    format!("Renamed collection to ‘{name}’")\n                }\n''',
    '''                CollectionAction::SetFaceDetection(id, enabled) => {\n                    face_scope::set_collection_enabled(&self.db_path, id, enabled)?;\n                    if enabled {\n                        "Face detection enabled for this collection".to_owned()\n                    } else {\n                        "Face detection disabled for this collection; existing face data was kept".to_owned()\n                    }\n                }\n''',
)
