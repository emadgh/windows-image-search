from pathlib import Path

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
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
