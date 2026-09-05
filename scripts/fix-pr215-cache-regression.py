from pathlib import Path


def ensure_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
replacements = [
    (
        "struct VisibleOrderKey {\n    image_catalog_revision: u64,\n",
        "struct VisibleOrderKey {\n    image_catalog_revision: u64,\n    similarity_results_revision: u64,\n",
        "cache key similarity revision",
    ),
    (
        "    pub(super) sort_mode: SortMode,\n    image_catalog_revision: u64,\n    visible_order_cache: RefCell<VisibleOrderCache>,\n",
        "    pub(super) sort_mode: SortMode,\n    image_catalog_revision: u64,\n    similarity_results_revision: u64,\n    visible_order_cache: RefCell<VisibleOrderCache>,\n",
        "app similarity revision field",
    ),
    (
        "            sort_mode: SortMode::Relevance,\n            image_catalog_revision: 0,\n            visible_order_cache: RefCell::new(VisibleOrderCache::default()),\n",
        "            sort_mode: SortMode::Relevance,\n            image_catalog_revision: 0,\n            similarity_results_revision: 0,\n            visible_order_cache: RefCell::new(VisibleOrderCache::default()),\n",
        "app similarity revision default",
    ),
    (
        "                WorkerMessage::SimilarityResults(results) => {\n                    self.similarity_results = Some(results);\n",
        "                WorkerMessage::SimilarityResults(results) => {\n                    self.similarity_results_revision =\n                        self.similarity_results_revision.wrapping_add(1);\n                    self.similarity_results = Some(results);\n",
        "similarity result revision bump",
    ),
    (
        "        VisibleOrderKey {\n            image_catalog_revision: self.image_catalog_revision,\n",
        "        VisibleOrderKey {\n            image_catalog_revision: self.image_catalog_revision,\n            similarity_results_revision: self.similarity_results_revision,\n",
        "cache key similarity revision value",
    ),
]
for old, new, label in replacements:
    text = ensure_once(text, old, new, label)
path.write_text(text, encoding="utf-8")

face_path = Path("src/ui/face_search_panel.rs")
face_text = face_path.read_text(encoding="utf-8")
old = "        self.similarity_results = Some(results);\n        self.query_image = Some(query_image);\n"
new = "        self.similarity_results_revision =\n            self.similarity_results_revision.wrapping_add(1);\n        self.similarity_results = Some(results);\n        self.query_image = Some(query_image);\n"
face_text = ensure_once(face_text, old, new, "face-search similarity result revision bump")
face_path.write_text(face_text, encoding="utf-8")
