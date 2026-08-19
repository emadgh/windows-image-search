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
    "Cargo.toml",
    'version = "0.2.6"',
    'version = "0.2.7"',
)

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

# The current UI/search path uses lightweight summaries and selective feature
# loaders, so remove two obsolete whole-dataset convenience APIs instead of
# suppressing their dead-code warnings.
p = Path("src/db.rs")
text = p.read_text(encoding="utf-8")
if "pub fn load_collection_effective_paths(" in text:
    start = text.find("pub fn load_collection_effective_paths(")
    end = text.find("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct FileState", start)
    if start < 0 or end < 0:
        raise SystemExit("src/db.rs: cannot locate obsolete load_collection_effective_paths block")
    text = text[:start] + text[end:]

if "pub fn load_images(db_path: &Path) -> Result<Vec<ImageRecord>> {" in text:
    start = text.find("pub fn load_images(db_path: &Path) -> Result<Vec<ImageRecord>> {")
    end = text.find("#[derive(Clone, Debug)]\npub struct AnnEmbedding", start)
    if start < 0 or end < 0:
        raise SystemExit("src/db.rs: cannot locate obsolete load_images block")
    text = text[:start] + text[end:]

old_record_tail = """            material_texture,
            embedding: None,
            embedding_normalized: row.get::<_, bool>(16)?,
            score: None,"""
new_record_tail = """            material_texture,
            score: None,"""
if old_record_tail in text:
    text = text.replace(old_record_tail, new_record_tail, 1)
elif new_record_tail not in text:
    raise SystemExit("src/db.rs: cannot locate search-record embedding tail")

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
if old_collection_test in text:
    text = text.replace(old_collection_test, new_collection_test, 1)
elif new_collection_test not in text:
    raise SystemExit("src/db.rs: cannot locate legacy collection effective-path test")

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

        // Heavy CLIP vectors stay out of UI/search records and are loaded only
        // for the rowids selected by the candidate stage.
        let rowids = std::collections::HashSet::from([search_records[0].rowid]);
        let embeddings = load_embeddings_for_rowids(&db_path, &rowids).unwrap();
        let (embedding, normalized) = embeddings.get(&search_records[0].rowid).unwrap();
        assert_eq!(embedding.len(), 4);
        assert!(*normalized);"""
if old_lightweight_test in text:
    text = text.replace(old_lightweight_test, new_lightweight_test, 1)
elif new_lightweight_test not in text:
    raise SystemExit("src/db.rs: cannot locate legacy full-record summary test")
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

# The image-model benchmark stores the transform on each query. Use that field
# directly rather than relying on query generation order, which also removes
# the dead-code warning introduced by the benchmark feature.
replace_once(
    "src/model_benchmark.rs",
    """    fn label(self) -> &'static str {
        match self {
            Self::CenterCrop80 => \"center_crop_80\",
            Self::OffsetCrop70 => \"offset_crop_70\",
            Self::HalfResolution => \"half_resolution\",
        }
    }
}""",
    """    fn label(self) -> &'static str {
        match self {
            Self::CenterCrop80 => \"center_crop_80\",
            Self::OffsetCrop70 => \"offset_crop_70\",
            Self::HalfResolution => \"half_resolution\",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::CenterCrop80 => 0,
            Self::OffsetCrop70 => 1,
            Self::HalfResolution => 2,
        }
    }
}""",
)
replace_once(
    "src/model_benchmark.rs",
    """    for (query_index, (query, embedding)) in queries.iter().zip(query_embeddings.iter()).enumerate()
    {
        let rank = rank_for_query(embedding, &corpus_embeddings, query.target_index);
        overall.record(rank);
        let variant_index = query_index % QUERY_VARIANTS.len();
        per_variant[variant_index].1.record(rank);
    }""",
    """    for (query, embedding) in queries.iter().zip(query_embeddings.iter()) {
        let rank = rank_for_query(embedding, &corpus_embeddings, query.target_index);
        overall.record(rank);
        per_variant[query.variant.index()].1.record(rank);
    }""",
)
