from pathlib import Path


def replace_exact(path: str, old: str, new: str, expected: int = 1) -> None:
    file = Path(path)
    text = file.read_text(encoding="utf-8")
    count = text.count(old)
    if count != expected:
        raise SystemExit(f"{path}: expected {expected} matches, found {count}")
    file.write_text(text.replace(old, new), encoding="utf-8")


replace_exact(
    "src/main.rs",
    "mod text_search;\nmod ui;",
    "mod text_search;\nmod thumbnail_cache;\nmod ui;",
)

replace_exact(
    "src/ui/mod.rs",
    "use crate::text_search::TextSearchService;",
    "use crate::text_search::TextSearchService;\nuse crate::thumbnail_cache;",
)
replace_exact(
    "src/ui/mod.rs",
    '        let thumbnail_cache = app_data_dir.join("thumbnail-cache");',
    "        let thumbnail_cache = thumbnail_cache::cache_dir_for_db(&db_path);",
)

thumb = Path("src/ui/thumbnails.rs")
text = thumb.read_text(encoding="utf-8")
old_imports = '''use image::codecs::jpeg::JpegEncoder;\nuse image::DynamicImage;\nuse std::collections::{hash_map::DefaultHasher, HashSet};\nuse std::fs::File;\nuse std::hash::{Hash, Hasher};\nuse std::io::BufWriter;\nuse std::path::{Path, PathBuf};\nuse std::sync::{\n    mpsc::{self, Receiver, Sender},\n    Arc, Mutex,\n};\nuse std::time::UNIX_EPOCH;\n\nconst CACHE_EDGE: u32 = 512;\n'''
new_imports = '''use crate::thumbnail_cache;\nuse image::DynamicImage;\nuse std::collections::HashSet;\nuse std::path::{Path, PathBuf};\nuse std::sync::{\n    mpsc::{self, Receiver, Sender},\n    Arc, Mutex,\n};\n'''
if text.count(old_imports) != 1:
    raise SystemExit("thumbnail imports: expected exactly one match")
text = text.replace(old_imports, new_imports)
start = text.index("fn load_or_build(")
to_rgba = text.index("fn to_rgba(", start)
replacement = '''fn load_or_build(cache_dir: &Path, source: &Path) -> Option<(usize, usize, Vec<u8>)> {\n    thumbnail_cache::load_or_build(cache_dir, source).map(to_rgba)\n}\n\n'''
text = text[:start] + replacement + text[to_rgba:]
cache_fn = text.find("\nfn thumbnail_cache_path(")
if cache_fn == -1:
    raise SystemExit("thumbnail_cache_path function not found")
text = text[:cache_fn].rstrip() + "\n"
thumb.write_text(text, encoding="utf-8")

replace_exact(
    "src/indexer.rs",
    "use crate::settings::IndexingSettings;",
    "use crate::settings::IndexingSettings;\nuse crate::thumbnail_cache;",
)
replace_exact(
    "src/indexer.rs",
    "    let indexing_settings = indexing_settings.sanitized();\n    let mut conn = db::open(db_path)?;",
    "    let indexing_settings = indexing_settings.sanitized();\n    let thumbnail_cache_dir = thumbnail_cache::cache_dir_for_db(db_path);\n    let mut conn = db::open(db_path)?;",
    expected=2,
)
replace_exact(
    "src/indexer.rs",
    "inspect_image(&item.path)",
    "inspect_image(&item.path, &thumbnail_cache_dir)",
    expected=2,
)
replace_exact(
    "src/indexer.rs",
    '''fn inspect_image(path: &Path) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>)> {\n    let image = decode_image(path)?;\n    let (width, height) = image.dimensions();\n    let (dominant, visual_hash, color_histogram) = visual_descriptor(&image);\n    Ok((width, height, dominant, visual_hash, color_histogram))\n}''',
    '''fn inspect_image(\n    path: &Path,\n    thumbnail_cache_dir: &Path,\n) -> Result<(u32, u32, [u8; 3], u64, Vec<f32>)> {\n    let image = decode_image(path)?;\n    let (width, height) = image.dimensions();\n\n    // Seed the exact same persistent cache used by the UI while the original\n    // file is already decoded. Thumbnail cache failures never invalidate the\n    // authoritative image index; the UI can still rebuild the preview later.\n    let _ = thumbnail_cache::store_from_decoded(thumbnail_cache_dir, path, &image);\n\n    let (dominant, visual_hash, color_histogram) = visual_descriptor(&image);\n    Ok((width, height, dominant, visual_hash, color_histogram))\n}''',
)
