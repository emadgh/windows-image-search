from pathlib import Path

path = Path("src/thumbnail_cache.rs")
text = path.read_text(encoding="utf-8")
needle = '''    #[test]
    fn portable_cache_is_written_inside_root_marker() {
'''
insert = '''    #[test]
    fn stale_temp_file_does_not_block_thumbnail_cache_recovery() {
        let dir = temp_dir("thumb-restart");
        let source_dir = dir.join("source");
        let cache_dir = dir.join("cache");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        let source = source_dir.join("large.png");
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(900, 700, Rgb([12, 34, 56])));
        image.save(&source).unwrap();

        let stale = cache_dir.join(".thumbnail-crash-leftover.tmp");
        std::fs::write(&stale, b"partial jpeg from interrupted writer").unwrap();

        let decoded = image::ImageReader::open(&source).unwrap().decode().unwrap();
        let committed = store_from_decoded(&cache_dir, &source, &decoded).unwrap();
        assert!(committed.is_file());
        assert!(stale.is_file(), "unrelated temp files must not be deleted because another writer may own them");

        let cached = load_cached(&cache_dir, &source).expect("committed cache should reload after stale temp");
        assert!(cached.width() <= CACHE_EDGE);
        assert!(cached.height() <= CACHE_EDGE);
        let _ = std::fs::remove_dir_all(dir);
    }

'''
if "fn stale_temp_file_does_not_block_thumbnail_cache_recovery()" in text:
    raise SystemExit(0)
if needle not in text:
    raise SystemExit("insertion anchor not found")
path.write_text(text.replace(needle, insert + needle, 1), encoding="utf-8")
