from pathlib import Path

path = Path("src/oversized_preview.rs")
text = path.read_text(encoding="utf-8")

old_cleanup = '''    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
'''
new_cleanup = '''    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let is_preview_temp = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with(".preview-") && name.ends_with(".tmp"))
            .unwrap_or(false);
        if is_preview_temp {
            // A concurrent writer may own this temp file. Unique temp names mean
            // stale leftovers cannot block a later commit, so leave them alone.
            continue;
        }
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
'''
if old_cleanup in text:
    text = text.replace(old_cleanup, new_cleanup, 1)
elif new_cleanup not in text:
    raise SystemExit("cleanup anchor not found")

needle = '''    #[test]
    fn source_cache_can_be_removed_after_source_deletion() {
'''
insert = '''    #[test]
    fn stale_preview_temp_does_not_block_or_get_deleted_by_cache_recovery() {
        let root = temp_root("restart");
        std::fs::create_dir_all(root.join("images")).unwrap();
        let source = root.join("images").join("large.jpg");
        DynamicImage::ImageRgb8(ImageBuffer::from_pixel(3200, 1800, Rgb([20, 40, 60])))
            .save(&source)
            .unwrap();
        let meta = std::fs::metadata(&source).unwrap();
        let modified = modified_seconds(&meta);
        let expected = cache_path_for_state(&root, &source, meta.len(), modified).unwrap();
        let cache_parent = expected.parent().unwrap();
        std::fs::create_dir_all(cache_parent).unwrap();
        let stale = cache_parent.join(".preview-crash-leftover.tmp");
        std::fs::write(&stale, b"partial previous-process write").unwrap();

        let first = load_or_build(&root, &source, meta.len(), modified).unwrap();
        assert!(first.path.is_file());
        assert!(stale.is_file(), "cleanup must not delete another writer's temp file");
        let first_size = image::image_dimensions(&first.path).unwrap();
        assert!(first_size.0 <= PREVIEW_EDGE && first_size.1 <= PREVIEW_EDGE);

        let second = load_or_build(&root, &source, meta.len(), modified).unwrap();
        assert!(second.reused);
        assert_eq!(first.path, second.path);
        assert!(stale.is_file());
        let _ = std::fs::remove_dir_all(root);
    }

'''
if "fn stale_preview_temp_does_not_block_or_get_deleted_by_cache_recovery()" not in text:
    if needle not in text:
        raise SystemExit("test insertion anchor not found")
    text = text.replace(needle, insert + needle, 1)

path.write_text(text, encoding="utf-8")
