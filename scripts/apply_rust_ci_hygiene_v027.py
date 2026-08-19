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

# The UI/search engine now uses lightweight summaries plus selective feature
# loaders. Remove the two legacy whole-dataset convenience loaders rather than
# keeping dead APIs around solely for old tests.
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

text = text.replace(
    """            material_texture,
            embedding: None,
            embedding_normalized: row.get::<_, bool>(16)?,
            score: None,""",
    """            material_texture,
            score: None,""",
    1,
)

old_collection_test = """        // first.jpg belongs both through the folder and explicitly, but appears once.
        let effective = load_collection_effective_paths(&db_path, collection.id).unwrap();
        assert_eq!(effective.len(), 3);
        assert!(effective.contains(&first));
        assert!(effective.contains(&nested));
        assert!(effective.contains(&manual));"""
new_collection_test = """        // Reconstruct the same effective membership used by the UI from the
        // persisted folder/file rules plus lightweight indexed summaries. A
        // file assigned both ways must still appear only once.
        let effective: std::collections::HashSet<PathBuf> = load_image_summaries(&db_path)
            .unwrap()
            .into_iter()
            .filter(|summary| {
                membership.files.iter().any(|file| file == &summary.path)
                    || membership
                        .folders
                        .iter()
                        .any(|folder| summary.path.starts_with(folder))
            })
            .map(|summary| summary.path)
            .collect();
        assert_eq!(effective.len(), 3);
        assert!(effective.contains(&first));
        assert!(effective.contains(&nested));
        assert!(effective.contains(&manual));"""
if old_collection_test not in text:
    raise SystemExit("src/db.rs: cannot locate legacy collection effective-path test")
text = text.replace(old_collection_test, new_collection_test, 1)

old_lightweight_test = """        let full = load_images(&db_path).unwrap();
        let summaries = load_image_summaries(&db_path).unwrap();
        assert_eq!(full.len(), 1);
        assert_eq!(summaries.len(), 1);
        assert!(full[0].embedding.is_some());
        assert!(full[0].embedding_normalized);
        assert!(full[0].color_histogram.is_some());
        assert_eq!(summaries[0].path, full[0].path);
        assert_eq!(summaries[0].file_name, full[0].file_name);
        assert_eq!(summaries[0].description, full[0].description);
        assert_eq!(summaries[0].dominant, full[0].dominant);"""
new_lightweight_test = """        let search_records = load_search_images(&db_path).unwrap();
        let summaries = load_image_summaries(&db_path).unwrap();
        assert_eq!(search_records.len(), 1);
        assert_eq!(summaries.len(), 1);
        assert!(search_records[0].color_histogram.is_some());
        assert_eq!(summaries[0].path, search_records[0].path);
        assert_eq!(summaries[0].file_name, search_records[0].file_name);
        assert_eq!(summaries[0].description, search_records[0].description);
        assert_eq!(summaries[0].dominant, search_records[0].dominant);

        // Heavy CLIP vectors stay out of both UI summaries and search records;
        // the search engine loads only the requested rowids on demand.
        let rowids = std::collections::HashSet::from([search_records[0].rowid]);
        let embeddings = load_embeddings_for_rowids(&db_path, &rowids).unwrap();
        let (embedding, normalized) = embeddings.get(&search_records[0].rowid).unwrap();
        assert_eq!(embedding.len(), 4);
        assert!(*normalized);"""
if old_lightweight_test not in text:
    raise SystemExit("src/db.rs: cannot locate legacy full-record summary test")
text = text.replace(old_lightweight_test, new_lightweight_test, 1)
p.write_text(text, encoding="utf-8")

replace_once(
    "src/indexer.rs",
    """        let records = db::load_images(&db_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, first);""",
    """        let records = db::load_image_summaries(&db_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].path, first);""",
)
