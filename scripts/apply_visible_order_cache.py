from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# ---- src/ui/collections.rs ----
path = Path("src/ui/collections.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "    face_detection: HashMap<i64, bool>,\n    new_name: String,\n",
    "    face_detection: HashMap<i64, bool>,\n    filter_revision: u64,\n    new_name: String,\n",
    "collection filter revision field",
)
text = once(
    text,
    "    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {\n        self.effective.clear();\n",
    "    pub(super) fn rebuild_effective(&mut self, images: &[ImageSummary]) {\n        self.filter_revision = self.filter_revision.wrapping_add(1);\n        self.effective.clear();\n",
    "collection revision bump",
)
text = once(
    text,
    "    pub(super) fn collection_filter_matches(&self, path: &Path) -> bool {\n        self.collections.filter_matches(path)\n    }\n\n",
    "    pub(super) fn collection_filter_matches(&self, path: &Path) -> bool {\n        self.collections.filter_matches(path)\n    }\n\n    pub(super) fn collection_filter_cache_token(&self) -> (Option<i64>, u64) {\n        (\n            self.collections.active_filter,\n            self.collections.filter_revision,\n        )\n    }\n\n",
    "collection cache token",
)
path.write_text(text, encoding="utf-8")


# ---- src/ui/people_filter.rs ----
path = Path("src/ui/people_filter.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "    pub(super) fn people_filter_selected_count(&self) -> usize {\n        self.people_filter_ui.selected_person_ids.len()\n    }\n\n",
    "    pub(super) fn people_filter_selected_count(&self) -> usize {\n        self.people_filter_ui.selected_person_ids.len()\n    }\n\n    pub(super) fn people_filter_cache_token(&self) -> (u64, bool) {\n        (\n            self.people_filter_ui.resolve_generation,\n            self.people_filter_ui.resolving,\n        )\n    }\n\n",
    "people cache token",
)
path.write_text(text, encoding="utf-8")


# ---- src/ui/mod.rs ----
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "use eframe::egui;\nuse egui::{ColorImage, TextureHandle, TextureOptions};\nuse std::collections::{HashMap, HashSet};\n",
    "use eframe::egui;\nuse egui::{ColorImage, TextureHandle, TextureOptions};\nuse std::cell::RefCell;\nuse std::collections::{HashMap, HashSet};\nuse std::hash::{Hash, Hasher};\n",
    "cache imports 1",
)
text = once(
    text,
    "use std::sync::mpsc::{Receiver, Sender};\n",
    "use std::sync::mpsc::{Receiver, Sender};\nuse std::sync::Arc;\n",
    "cache imports 2",
)

marker = '''impl SortMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Relevance => "Relevance",
            Self::Name => "Name",
            Self::Modified => "Modified",
            Self::Size => "File size",
            Self::Resolution => "Resolution",
        }
    }
}
'''
addition = marker + '''
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleOrderKey {
    image_catalog_revision: u64,
    source_ptr: usize,
    source_len: usize,
    source_is_similarity: bool,
    search_text_hash: u64,
    text_search_generation: u64,
    text_search_pending: bool,
    text_match_count: usize,
    collection_filter: Option<i64>,
    collection_revision: u64,
    people_resolve_generation: u64,
    people_resolving: bool,
    color_enabled: bool,
    target_color: [u8; 3],
    color_tolerance_bits: u32,
    sort_mode: SortMode,
}

struct VisibleOrderCache {
    key: Option<VisibleOrderKey>,
    indices: Arc<[usize]>,
}

impl Default for VisibleOrderCache {
    fn default() -> Self {
        Self {
            key: None,
            indices: Vec::<usize>::new().into(),
        }
    }
}

fn sort_indices_by_cached_name<F>(indices: &mut [usize], mut key: F)
where
    F: FnMut(usize) -> String,
{
    indices.sort_by_cached_key(|index| key(*index));
}
'''
text = once(text, marker, addition, "visible cache types")

text = once(
    text,
    "    pub(super) sort_mode: SortMode,\n    pub(super) inspector_open: bool,\n",
    "    pub(super) sort_mode: SortMode,\n    image_catalog_revision: u64,\n    visible_order_cache: RefCell<VisibleOrderCache>,\n    pub(super) inspector_open: bool,\n",
    "cache app fields",
)
text = once(
    text,
    "            sort_mode: SortMode::Relevance,\n            inspector_open: true,\n",
    "            sort_mode: SortMode::Relevance,\n            image_catalog_revision: 0,\n            visible_order_cache: RefCell::new(VisibleOrderCache::default()),\n            inspector_open: true,\n",
    "cache app defaults",
)

text = once(
    text,
    "    fn rebuild_image_positions(&mut self) {\n        self.image_positions.clear();\n",
    "    fn rebuild_image_positions(&mut self) {\n        self.image_catalog_revision = self.image_catalog_revision.wrapping_add(1);\n        self.image_positions.clear();\n",
    "catalog revision rebuild",
)
text = once(
    text,
    "    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {\n        for record in records {\n",
    "    fn merge_indexed_batch(&mut self, records: Vec<ImageSummary>) {\n        self.image_catalog_revision = self.image_catalog_revision.wrapping_add(1);\n        for record in records {\n",
    "catalog revision merge",
)
text = once(
    text,
    "    fn remove_indexed_paths(&mut self, paths: Vec<PathBuf>) {\n        if paths.is_empty() {\n            return;\n        }\n",
    "    fn remove_indexed_paths(&mut self, paths: Vec<PathBuf>) {\n        if paths.is_empty() {\n            return;\n        }\n        self.image_catalog_revision = self.image_catalog_revision.wrapping_add(1);\n",
    "catalog revision remove",
)

start = text.find("    pub(super) fn visible_indices(&self) -> Vec<usize> {\n")
if start < 0:
    raise SystemExit("missing visible_indices start")
end = text.find(
    "    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {\n",
    start,
)
if end < 0:
    raise SystemExit("missing visible_indices end")
replacement = '''    fn visible_order_key(&self) -> VisibleOrderKey {
        let source = self.source();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.search_text.hash(&mut hasher);
        let (collection_filter, collection_revision) = self.collection_filter_cache_token();
        let (people_resolve_generation, people_resolving) = self.people_filter_cache_token();
        VisibleOrderKey {
            image_catalog_revision: self.image_catalog_revision,
            source_ptr: source.as_ptr() as usize,
            source_len: source.len(),
            source_is_similarity: self.similarity_results.is_some(),
            search_text_hash: hasher.finish(),
            text_search_generation: self.text_search_generation,
            text_search_pending: self.text_search_pending,
            text_match_count: self.text_search_matches.as_ref().map_or(0, HashSet::len),
            collection_filter,
            collection_revision,
            people_resolve_generation,
            people_resolving,
            color_enabled: self.color_enabled,
            target_color: self.target_color,
            color_tolerance_bits: self.color_tolerance.to_bits(),
            sort_mode: self.sort_mode,
        }
    }

    pub(super) fn visible_indices(&self) -> Arc<[usize]> {
        let key = self.visible_order_key();
        if let Some(indices) = {
            let cache = self.visible_order_cache.borrow();
            (cache.key == Some(key)).then(|| Arc::clone(&cache.indices))
        } {
            return indices;
        }

        let text_filter_active = !self.search_text.trim().is_empty();
        let source = self.source();
        let mut visible = source
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if !self.collection_filter_matches(&record.path) {
                    return false;
                }
                if !self.people_filter_matches(&record.path) {
                    return false;
                }
                if text_filter_active
                    && !self
                        .text_search_matches
                        .as_ref()
                        .is_some_and(|paths| paths.contains(&record.path))
                {
                    return false;
                }
                !self.color_enabled
                    || crate::indexer::color_distance(record.dominant, self.target_color)
                        <= self.color_tolerance
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        match self.sort_mode {
            SortMode::Relevance => {}
            SortMode::Name => sort_indices_by_cached_name(&mut visible, |index| {
                source[index].file_name.to_lowercase()
            }),
            SortMode::Modified => visible.sort_by(|a, b| {
                source[*b]
                    .modified
                    .cmp(&source[*a].modified)
                    .then_with(|| source[*a].file_name.cmp(&source[*b].file_name))
            }),
            SortMode::Size => visible.sort_by(|a, b| {
                source[*b]
                    .size
                    .cmp(&source[*a].size)
                    .then_with(|| source[*a].file_name.cmp(&source[*b].file_name))
            }),
            SortMode::Resolution => visible.sort_by(|a, b| {
                let left = u64::from(source[*a].width) * u64::from(source[*a].height);
                let right = u64::from(source[*b].width) * u64::from(source[*b].height);
                right
                    .cmp(&left)
                    .then_with(|| source[*a].file_name.cmp(&source[*b].file_name))
            }),
        }

        let indices: Arc<[usize]> = visible.into();
        let mut cache = self.visible_order_cache.borrow_mut();
        cache.key = Some(key);
        cache.indices = Arc::clone(&indices);
        indices
    }

'''
text = text[:start] + replacement + text[end:]

text = once(
    text,
    '''    pub(super) fn visible_result_paths(&self) -> Vec<PathBuf> {
        let indices = self.visible_indices();
        let source = self.source();
        indices
            .into_iter()
            .filter_map(|index| source.get(index).map(|record| record.path.clone()))
            .collect()
    }
''',
    '''    pub(super) fn visible_result_paths(&self) -> Vec<PathBuf> {
        let indices = self.visible_indices();
        let source = self.source();
        indices
            .iter()
            .copied()
            .filter_map(|index| source.get(index).map(|record| record.path.clone()))
            .collect()
    }
''',
    "visible result paths arc iteration",
)

text += '''

#[cfg(test)]
mod visible_order_tests {
    use super::sort_indices_by_cached_name;

    #[test]
    fn cached_name_sort_handles_large_synthetic_result_set() {
        const COUNT: usize = 50_000;
        let names = (0..COUNT)
            .map(|index| format!("IMG_{:05}.JPG", COUNT - index))
            .collect::<Vec<_>>();
        let mut indices = (0..COUNT).collect::<Vec<_>>();

        sort_indices_by_cached_name(&mut indices, |index| names[index].to_lowercase());

        assert_eq!(indices.first().copied(), Some(COUNT - 1));
        assert_eq!(indices.last().copied(), Some(0));
        assert!(indices.windows(2).all(|pair| {
            names[pair[0]].to_lowercase() <= names[pair[1]].to_lowercase()
        }));
    }
}
'''
path.write_text(text, encoding="utf-8")
