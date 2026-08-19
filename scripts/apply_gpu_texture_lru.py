from pathlib import Path

path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")


def replace_once(old: str, new: str) -> None:
    global text
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"expected one match, found {count}: {old[:80]!r}")
    text = text.replace(old, new, 1)


replace_once(
    "mod thumbnails;\nmod views;",
    "mod thumbnails;\nmod texture_lru;\nmod views;",
)

replace_once(
    "use thumbnails::{ThumbnailPool, ThumbnailResult};",
    "use texture_lru::{TextureLru, DEFAULT_GPU_TEXTURE_CAPACITY};\nuse thumbnails::{ThumbnailPool, ThumbnailResult};",
)

replace_once(
    "    pub(super) textures: HashMap<PathBuf, TextureHandle>,\n    pub(super) selected_paths: HashSet<PathBuf>,",
    "    pub(super) textures: HashMap<PathBuf, TextureHandle>,\n    texture_lru: TextureLru,\n    pub(super) selected_paths: HashSet<PathBuf>,",
)

replace_once(
    "            textures: HashMap::new(),\n            selected_paths: HashSet::new(),",
    "            textures: HashMap::new(),\n            texture_lru: TextureLru::new(DEFAULT_GPU_TEXTURE_CAPACITY),\n            selected_paths: HashSet::new(),",
)

replace_once(
    "        for record in records {\n            self.textures.remove(&record.path);",
    "        for record in records {\n            self.textures.remove(&record.path);\n            self.texture_lru.remove(&record.path);",
)

replace_once(
    "        for path in &removed {\n            self.textures.remove(path);\n            self.selected_paths.remove(path);",
    "        for path in &removed {\n            self.textures.remove(path);\n            self.texture_lru.remove(path);\n            self.selected_paths.remove(path);",
)

replace_once(
    "                self.textures.insert(path, texture);",
    "                self.textures.insert(path.clone(), texture);\n                self.texture_lru.register(&path);",
)

replace_once(
    "        if received {\n            ctx.request_repaint();\n        }\n    }\n\n    fn start_rescan",
    "        if received {\n            self.evict_gpu_textures();\n            ctx.request_repaint();\n        }\n    }\n\n    fn evict_gpu_textures(&mut self) {\n        if self.textures.len() <= self.texture_lru.capacity() {\n            return;\n        }\n\n        let residents: Vec<PathBuf> = self.textures.keys().cloned().collect();\n        let mut protected = HashSet::new();\n        if let Some(query) = &self.query_image {\n            if self.textures.contains_key(query) {\n                protected.insert(query.clone());\n            }\n        }\n\n        for path in self\n            .texture_lru\n            .eviction_victims(&residents, &protected)\n        {\n            self.textures.remove(&path);\n        }\n    }\n\n    fn start_rescan",
)

replace_once(
    "    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {\n        if let Some(texture) = self.textures.get(path) {\n            return Some(texture.clone());\n        }\n        self.thumb_pool.request(path);\n        None\n    }",
    "    pub(super) fn thumbnail(&mut self, path: &Path) -> Option<TextureHandle> {\n        if let Some(texture) = self.textures.get(path).cloned() {\n            self.texture_lru.touch(path);\n            return Some(texture);\n        }\n        self.thumb_pool.request(path);\n        None\n    }",
)

replace_once(
    "    fn clear_thumbnail_cache(&mut self) {\n        self.textures.clear();\n        self.thumb_pool.clear_cache();",
    "    fn clear_thumbnail_cache(&mut self) {\n        self.textures.clear();\n        self.texture_lru.clear();\n        self.thumb_pool.clear_cache();",
)

replace_once(
    "                ui.label(\n                    \"Cached previews are generated at up to 512 px on background worker threads.\",\n                );\n                if ui.button(\"Clear thumbnail cache\").clicked() {",
    "                ui.label(\n                    \"Cached previews are generated at up to 512 px on background worker threads.\",\n                );\n                ui.small(format!(\n                    \"GPU/UI thumbnail textures: {} / {} active (LRU bounded; disk cache survives eviction).\",\n                    self.textures.len(),\n                    self.texture_lru.capacity()\n                ));\n                if ui.button(\"Clear thumbnail cache\").clicked() {",
)

path.write_text(text, encoding="utf-8")
