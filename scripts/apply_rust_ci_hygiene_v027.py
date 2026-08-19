from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/ui/collections.rs",
    "egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)",
    "egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color)",
)
replace_once(
    "src/ui/views.rs",
    """                                            egui::Stroke::new(
                                                2.0,
                                                ui.visuals().selection.stroke.color,
                                            ),""",
    """                                            egui::Stroke::new(
                                                2.0_f32,
                                                ui.visuals().selection.stroke.color,
                                            ),""",
)
replace_once(
    "src/ui/views.rs",
    "egui::Stroke::new(3.0, ui.visuals().selection.stroke.color)",
    "egui::Stroke::new(3.0_f32, ui.visuals().selection.stroke.color)",
)
replace_once(
    "src/indexer.rs",
    "let mut records = db::load_search_images(db_path)?;",
    "let records = db::load_search_images(db_path)?;",
)
replace_once(
    "src/ui/mod.rs",
    "    pub(super) collections: collections::CollectionsState,",
    "    collections: collections::CollectionsState,",
)
replace_once(
    "src/db.rs",
    """    pub material_texture: Option<Vec<f32>>,
    pub embedding: Option<Vec<f32>>,
    pub embedding_normalized: bool,
    pub score: Option<f32>,""",
    """    pub material_texture: Option<Vec<f32>>,
    pub score: Option<f32>,""",
)

p = Path("src/db.rs")
text = p.read_text(encoding="utf-8")
start = text.find("pub fn load_collection_effective_paths(")
end = text.find("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct FileState", start)
if start < 0 or end < 0:
    raise SystemExit("src/db.rs: cannot locate obsolete load_collection_effective_paths block")
text = text[:start] + text[end:]

start = text.find("pub fn load_images(db_path: &Path) -> Result<Vec<ImageRecord>> {")
end = text.find("#[derive(Clone, Debug)]\npub struct AnnEmbedding", start)
if start < 0 or end < 0:
    raise SystemExit("src/db.rs: cannot locate obsolete load_images block")
text = text[:start] + text[end:]

text = text.replace("""            material_texture,
            embedding: None,
            embedding_normalized: row.get::<_, bool>(16)?,
            score: None,""", """            material_texture,
            score: None,""", 1)
p.write_text(text, encoding="utf-8")
