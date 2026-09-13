from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if new in text:
        return text
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


preview_path = Path("src/oversized_preview.rs")
preview = preview_path.read_text(encoding="utf-8")
preview = replace_once(
    preview,
    "use crate::{portable, thumbnail_cache};",
    "use crate::{portable, settings, thumbnail_cache};",
    "oversized_preview imports",
)
preview = replace_once(
    preview,
    "const JPEG_QUALITY: u8 = 88;\nconst MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;",
    "const JPEG_QUALITY: u8 = 88;\npub const MAX_DIRECT_DECODE_BYTES: u64 = 256 * 1024 * 1024;\nconst ESTIMATED_DECODE_BYTES_PER_PIXEL: u64 = 8;\nconst MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;",
    "decode ceiling constants",
)
preview = replace_once(
    preview,
    "pub fn bounded_backend_for(source: &Path) -> Result<BoundedBackend> {",
    '''pub fn estimated_decode_bytes(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(ESTIMATED_DECODE_BYTES_PER_PIXEL)
}

pub fn requires_bounded_dimensions(source_size: u64, width: u32, height: u32) -> bool {
    source_size > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES
        || estimated_decode_bytes(width, height) > MAX_DIRECT_DECODE_BYTES
}

pub fn requires_bounded_decode(source: &Path, source_size: u64) -> Result<bool> {
    if source_size > settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES {
        return Ok(true);
    }
    let (width, height) = source_dimensions(source)?;
    Ok(requires_bounded_dimensions(source_size, width, height))
}

pub fn bounded_backend_for(source: &Path) -> Result<BoundedBackend> {''',
    "bounded routing helpers",
)
preview = replace_once(
    preview,
    "    fn jpeg_keeps_existing_scaled_decoder_backend() {",
    '''    fn decoded_memory_ceiling_routes_highly_compressed_sources_to_bounded_decode() {
        assert!(!requires_bounded_dimensions(8 * 1024 * 1024, 3_000, 2_000));
        assert!(requires_bounded_dimensions(8 * 1024 * 1024, 10_000, 10_000));
        assert!(requires_bounded_dimensions(
            settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES + 1,
            100,
            100
        ));
        assert_eq!(estimated_decode_bytes(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn jpeg_keeps_existing_scaled_decoder_backend() {''',
    "bounded routing tests",
)
preview_path.write_text(preview, encoding="utf-8")

ui_path = Path("src/ui/image_preview.rs")
ui = ui_path.read_text(encoding="utf-8")
ui = replace_once(
    ui,
    "const PREVIEW_EDGE: u32 = 2_560;\nconst MAX_DIRECT_DECODE_BYTES: u64 = 256 * 1024 * 1024;",
    "const PREVIEW_EDGE: u32 = 2_560;",
    "preview duplicate memory ceiling",
)
ui = replace_once(
    ui,
    '''    let decoded_bytes = u64::from(dimensions.0)
        .saturating_mul(u64::from(dimensions.1))
        // Some supported inputs decode to 16-bit RGBA before conversion.
        .saturating_mul(8);
    let direct = metadata.len() <= settings::DIRECT_DECODE_MAX_FILE_SIZE_BYTES
        && decoded_bytes <= MAX_DIRECT_DECODE_BYTES;''',
    '''    let direct = !oversized_preview::requires_bounded_dimensions(
        metadata.len(),
        dimensions.0,
        dimensions.1,
    );''',
    "preview bounded routing",
)
ui_path.write_text(ui, encoding="utf-8")

indexer_path = Path("src/indexer.rs")
indexer = indexer_path.read_text(encoding="utf-8")
indexer = replace_once(
    indexer,
    "    if item.size > DIRECT_DECODE_MAX_FILE_SIZE_BYTES {",
    "    if oversized_preview::requires_bounded_decode(&item.path, item.size)? {",
    "pending image routing",
)
indexer = replace_once(
    indexer,
    '''    if source_size > DIRECT_DECODE_MAX_FILE_SIZE_BYTES {
        bail!(
            "direct image decoder refused oversized source {}; resized preview is mandatory",
            path.display()
        );
    }''',
    '''    if oversized_preview::requires_bounded_decode(path, source_size)? {
        bail!(
            "direct image decoder refused memory-unsafe source {}; bounded preview is mandatory",
            path.display()
        );
    }''',
    "inspect image guard",
)
indexer = replace_once(
    indexer,
    "    if source_size > DIRECT_DECODE_MAX_FILE_SIZE_BYTES {\n        db::set_descriptor_provenance(",
    "    if oversized_preview::requires_bounded_decode(path, source_size)? {\n        db::set_descriptor_provenance(",
    "descriptor provenance routing",
)
indexer = replace_once(
    indexer,
    "pub(crate) fn safe_descriptor_image(",
    '''fn bounded_preview_asset_for_path(
    path: &Path,
    roots: &[PathBuf],
    meta: &std::fs::Metadata,
) -> Result<oversized_preview::PreviewAsset> {
    let root = indexed_root_for_path(path, roots).with_context(|| {
        format!(
            "memory-unsafe source {} is outside an indexed root; safe-preview processing is unavailable",
            path.display()
        )
    })?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    oversized_preview::load_or_build(root, path, meta.len(), modified)
}

pub(crate) fn safe_descriptor_image(''',
    "bounded asset helper",
)
old_descriptor = '''    match indexing_settings.source_policy(meta.len()) {
        SourcePolicy::SkipConfigured => bail!(
            "source {} exceeds configured {} MiB indexing limit",
            path.display(),
            indexing_settings.max_file_size_mib
        ),
        SourcePolicy::DirectSource => Ok((decode_image(path)?, path.to_path_buf(), false)),
        SourcePolicy::OversizedPreview => {
            let root = indexed_root_for_path(path, roots).with_context(|| {
                format!(
                    "oversized source {} is outside an indexed root; safe-preview processing is unavailable",
                    path.display()
                )
            })?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            let asset = oversized_preview::load_or_build(root, path, meta.len(), modified)?;
            Ok((asset.image, asset.path, true))
        }
    }'''
new_descriptor = '''    let policy = indexing_settings.source_policy(meta.len());
    let needs_bounded = matches!(policy, SourcePolicy::OversizedPreview)
        || (matches!(policy, SourcePolicy::DirectSource)
            && oversized_preview::requires_bounded_decode(path, meta.len())?);
    match policy {
        SourcePolicy::SkipConfigured => bail!(
            "source {} exceeds configured {} MiB indexing limit",
            path.display(),
            indexing_settings.max_file_size_mib
        ),
        SourcePolicy::DirectSource | SourcePolicy::OversizedPreview if needs_bounded => {
            let asset = bounded_preview_asset_for_path(path, roots, &meta)?;
            Ok((asset.image, asset.path, true))
        }
        SourcePolicy::DirectSource => Ok((decode_image(path)?, path.to_path_buf(), false)),
        SourcePolicy::OversizedPreview => unreachable!("oversized policy must use bounded decode"),
    }'''
indexer = replace_once(indexer, old_descriptor, new_descriptor, "safe descriptor routing")
old_embedding = '''    match indexing_settings.source_policy(meta.len()) {
        SourcePolicy::SkipConfigured => bail!(
            "source {} exceeds configured {} MiB indexing limit",
            path.display(),
            indexing_settings.max_file_size_mib
        ),
        SourcePolicy::OversizedPreview => {
            let root = indexed_root_for_path(path, roots).with_context(|| {
                format!(
                    "oversized indexed source has no registered root: {}",
                    path.display()
                )
            })?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            Ok(oversized_preview::load_or_build(root, path, meta.len(), modified)?.path)
        }
        SourcePolicy::DirectSource if prefer_thumbnail => Ok(indexed_root_for_path(path, roots)
            .and_then(|root| thumbnail_cache::valid_cache_path_for_root(root, path))
            .unwrap_or_else(|| path.to_path_buf())),
        SourcePolicy::DirectSource => Ok(path.to_path_buf()),
    }'''
new_embedding = '''    let policy = indexing_settings.source_policy(meta.len());
    let needs_bounded = matches!(policy, SourcePolicy::OversizedPreview)
        || (matches!(policy, SourcePolicy::DirectSource)
            && oversized_preview::requires_bounded_decode(path, meta.len())?);
    match policy {
        SourcePolicy::SkipConfigured => bail!(
            "source {} exceeds configured {} MiB indexing limit",
            path.display(),
            indexing_settings.max_file_size_mib
        ),
        SourcePolicy::DirectSource | SourcePolicy::OversizedPreview if needs_bounded => {
            Ok(bounded_preview_asset_for_path(path, roots, &meta)?.path)
        }
        SourcePolicy::DirectSource if prefer_thumbnail => Ok(indexed_root_for_path(path, roots)
            .and_then(|root| thumbnail_cache::valid_cache_path_for_root(root, path))
            .unwrap_or_else(|| path.to_path_buf())),
        SourcePolicy::DirectSource => Ok(path.to_path_buf()),
        SourcePolicy::OversizedPreview => unreachable!("oversized policy must use bounded decode"),
    }'''
indexer = replace_once(indexer, old_embedding, new_embedding, "safe embedding routing")
old_decode = '''fn decode_image(path: &Path) -> Result<DynamicImage> {
    if std::fs::metadata(path)
        .map(|meta| meta.len() > DIRECT_DECODE_MAX_FILE_SIZE_BYTES)
        .unwrap_or(false)
    {
        bail!(
            "direct decoder refused source above 256 MiB: {}",
            path.display()
        );
    }'''
new_decode = '''fn decode_image(path: &Path) -> Result<DynamicImage> {
    let source_size = std::fs::metadata(path)
        .with_context(|| format!("reading source metadata {}", path.display()))?
        .len();
    if oversized_preview::requires_bounded_decode(path, source_size)? {
        bail!(
            "direct decoder refused memory-unsafe source; bounded preview is required: {}",
            path.display()
        );
    }'''
indexer = replace_once(indexer, old_decode, new_decode, "direct decode guard")
indexer_path.write_text(indexer, encoding="utf-8")
