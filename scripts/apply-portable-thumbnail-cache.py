from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# src/ui/thumbnails.rs
path = Path("src/ui/thumbnails.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "pub struct ThumbnailPool {\n    fallback_cache_dir: PathBuf,\n",
    "pub struct ThumbnailPool {\n    // Kept only long enough to migrate caches created by older builds. New\n    // thumbnails must never be persisted here.\n    legacy_cache_dir: PathBuf,\n",
    "thumbnail pool legacy field",
)
text = replace_once(
    text,
    "    pub fn new(fallback_cache_dir: PathBuf, roots: Vec<PathBuf>) -> Self {\n        let _ = std::fs::create_dir_all(&fallback_cache_dir);\n",
    "    pub fn new(legacy_cache_dir: PathBuf, roots: Vec<PathBuf>) -> Self {\n",
    "thumbnail pool constructor",
)
text = replace_once(
    text,
    "            let fallback_cache = fallback_cache_dir.clone();\n            std::thread::spawn(move || loop {",
    "            std::thread::spawn(move || loop {",
    "worker fallback clone",
)
text = replace_once(
    text,
    "                let result = match load_or_build(&fallback_cache, &roots, &job.path) {",
    "                let result = match load_or_build(&roots, &job.path) {",
    "worker load call",
)
text = replace_once(
    text,
    "        Self {\n            fallback_cache_dir,\n",
    "        Self {\n            legacy_cache_dir,\n",
    "thumbnail pool construction field",
)
old_clear = '''    pub fn clear_cache(&mut self) {
        let _ = std::fs::remove_dir_all(&self.fallback_cache_dir);
        let _ = std::fs::create_dir_all(&self.fallback_cache_dir);
        if let Ok(roots) = self.roots.read() {
            for root in roots.iter() {
                let cache = portable::thumbnail_dir(root);
                let _ = std::fs::remove_dir_all(&cache);
                let _ = std::fs::create_dir_all(cache);
            }
        }
        let (lock, wake) = &*self.scheduler;
        if let Ok(mut state) = lock.lock() {
            state.clear();
            wake.notify_all();
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.fallback_cache_dir
    }
}

fn load_or_build(
    fallback_cache: &Path,
    roots: &RwLock<Vec<PathBuf>>,
    source: &Path,
) -> Option<(usize, usize, Vec<u8>)> {
'''
new_clear = '''    pub fn clear_cache(&mut self) {
        // A legacy AppData cache may still exist after upgrading. Remove it, but
        // never recreate it: all persistent thumbnails now belong to a root.
        let _ = std::fs::remove_dir_all(&self.legacy_cache_dir);
        if let Ok(roots) = self.roots.read() {
            for root in roots.iter() {
                let cache = portable::thumbnail_dir(root);
                let _ = std::fs::remove_dir_all(&cache);
                let _ = std::fs::create_dir_all(cache);
            }
        }
        let (lock, wake) = &*self.scheduler;
        if let Ok(mut state) = lock.lock() {
            state.clear();
            wake.notify_all();
        }
    }

    /// Delete the old AppData thumbnail cache after portable-root migration has
    /// completed. This is intentionally separate from construction because
    /// startup may still need the old cache as a migration source.
    pub fn retire_legacy_cache(&self) {
        let _ = std::fs::remove_dir_all(&self.legacy_cache_dir);
    }

    pub fn cache_dirs(&self) -> Vec<PathBuf> {
        self.roots
            .read()
            .map(|roots| {
                roots
                    .iter()
                    .filter(|root| portable::is_indexed_root(root))
                    .map(|root| portable::thumbnail_dir(root))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn load_or_build(
    roots: &RwLock<Vec<PathBuf>>,
    source: &Path,
) -> Option<(usize, usize, Vec<u8>)> {
'''
text = replace_once(text, old_clear, new_clear, "clear/cache dirs/load signature")
text = replace_once(
    text,
    "        _ => thumbnail_cache::load_or_build(fallback_cache, source),\n",
    "        // Query/reference images outside attached indexed roots are transient.\n        // Decode a small in-memory preview, but do not leave a cache on C:/AppData.\n        _ => load_transient(source),\n",
    "fallback branch",
)
insert_before = '''fn to_rgba(image: DynamicImage) -> (usize, usize, Vec<u8>) {
'''
transient = '''fn load_transient(source: &Path) -> Option<DynamicImage> {
    if std::fs::metadata(source).ok()?.len() > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES {
        return None;
    }
    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    Some(image.thumbnail(thumbnail_cache::CACHE_EDGE, thumbnail_cache::CACHE_EDGE))
}

'''
text = replace_once(text, insert_before, transient + insert_before, "transient thumbnail helper")
test_anchor = '''    #[test]
    fn newest_viewport_request_is_popped_first() {
'''
new_tests = '''    fn temp_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wis-thumbnail-pool-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn constructor_does_not_create_legacy_appdata_cache() {
        let legacy = temp_dir("legacy-not-created");
        assert!(!legacy.exists());
        let pool = ThumbnailPool::new(legacy.clone(), Vec::new());
        assert!(!legacy.exists());
        drop(pool);
    }

    #[test]
    fn transient_thumbnail_does_not_write_legacy_cache() {
        use image::{ImageBuffer, Rgb};
        let dir = temp_dir("transient");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("query.png");
        image::DynamicImage::ImageRgb8(ImageBuffer::from_pixel(900, 700, Rgb([4, 5, 6])))
            .save(&source)
            .unwrap();
        let legacy = dir.join("legacy-cache");
        let roots = RwLock::new(Vec::new());
        let (width, height, _) = load_or_build(&roots, &source).unwrap();
        assert!(width <= thumbnail_cache::CACHE_EDGE as usize);
        assert!(height <= thumbnail_cache::CACHE_EDGE as usize);
        assert!(!legacy.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retire_legacy_cache_removes_upgrade_leftovers() {
        let legacy = temp_dir("legacy-retire");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("old.jpg"), b"old").unwrap();
        let pool = ThumbnailPool::new(legacy.clone(), Vec::new());
        pool.retire_legacy_cache();
        assert!(!legacy.exists());
    }

'''
text = replace_once(text, test_anchor, new_tests + test_anchor, "thumbnail tests")
path.write_text(text, encoding="utf-8")


# src/ui/mod.rs
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "        let thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);\n",
    "        // This path is retained only so older AppData thumbnail caches can be\n        // migrated into each root's portable `.imagesearch/thumbnails` folder.\n        let legacy_thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);\n",
    "legacy thumbnail variable",
)
text = replace_once(
    text,
    "        let thumb_pool = ThumbnailPool::new(thumbnail_cache, Vec::new());\n",
    "        let thumb_pool = ThumbnailPool::new(legacy_thumbnail_cache, Vec::new());\n",
    "thumbnail pool constructor call",
)
text = replace_once(
    text,
    "                    self.thumb_pool.set_roots(self.roots.clone());\n                    self.fs_watch_service.set_roots(self.roots.clone());\n",
    "                    self.thumb_pool.set_roots(self.roots.clone());\n                    // `prepare_registered_roots` has already copied any legacy\n                    // AppData thumbnails into portable roots. The local cache can\n                    // now be retired and must not be recreated.\n                    self.thumb_pool.retire_legacy_cache();\n                    self.fs_watch_service.set_roots(self.roots.clone());\n",
    "retire cache after migration",
)
path.write_text(text, encoding="utf-8")


# src/ui/settings_window.rs
path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")
old_storage = '''    section_title(
        ui,
        "Storage",
        "Inspect local application state and manage disposable thumbnail cache data.",
    );
'''
new_storage = '''    section_title(
        ui,
        "Storage",
        "Inspect application state and manage portable per-library thumbnail caches.",
    );
'''
text = replace_once(text, old_storage, new_storage, "storage description")
old_thumb_ui = '''    ui.add_space(12.0);
    ui.strong("Thumbnail cache");
    ui.label(format!(
        "Location: {}",
        app.thumb_pool.cache_dir().display()
    ));
    ui.label("Cached previews are generated at up to 512 px on background worker threads.");
    ui.small(format!(
        "GPU/UI thumbnail textures: {} / {} active (LRU bounded; disk cache survives eviction).",
        app.textures.len(),
        app.texture_lru.capacity()
    ));
    if ui.button("Clear thumbnail cache").clicked() {
        effects.clear_cache = true;
    }
'''
new_thumb_ui = '''    ui.add_space(12.0);
    ui.strong("Thumbnail cache");
    ui.label("Persistent thumbnails are stored with each indexed folder under `.imagesearch/thumbnails`.");
    let thumbnail_dirs = app.thumb_pool.cache_dirs();
    if thumbnail_dirs.is_empty() {
        ui.small("No portable thumbnail cache is attached yet.");
    } else {
        for cache in thumbnail_dirs {
            ui.small(cache.display().to_string());
        }
    }
    ui.small("Images outside indexed roots are decoded transiently for preview and are not cached to AppData/C:.");
    ui.label("Cached previews are generated at up to 512 px on background worker threads.");
    ui.small(format!(
        "GPU/UI thumbnail textures: {} / {} active (LRU bounded; portable disk cache survives eviction).",
        app.textures.len(),
        app.texture_lru.capacity()
    ));
    if ui.button("Clear thumbnail cache").clicked() {
        effects.clear_cache = true;
    }
'''
text = replace_once(text, old_thumb_ui, new_thumb_ui, "portable storage UI")
path.write_text(text, encoding="utf-8")
