from pathlib import Path

path = Path('src/indexer.rs')
text = path.read_text(encoding='utf-8')

text = text.replace(
    'use image::{imageops::FilterType, DynamicImage, GenericImageView, Pixel};\nuse std::path::{Path, PathBuf};\nuse std::sync::mpsc::Sender;\n',
    'use image::{imageops::FilterType, DynamicImage, GenericImageView, Pixel};\nuse rayon::prelude::*;\nuse std::path::{Path, PathBuf};\nuse std::sync::atomic::{AtomicUsize, Ordering};\nuse std::sync::mpsc::Sender;\n',
    1,
)

marker = 'const COLOR_HISTOGRAM_BINS: usize = 64;\n'
insert = '''const COLOR_HISTOGRAM_BINS: usize = 64;

#[derive(Clone)]
struct PendingImage {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: i64,
}

struct PreparedImage {
    root: PathBuf,
    path: PathBuf,
    file_name: String,
    extension: String,
    size: u64,
    modified: i64,
    width: u32,
    height: u32,
    description: String,
    keywords: String,
    dominant: [u8; 3],
    visual_hash: u64,
    color_histogram: Vec<f32>,
}

fn background_worker_count() -> usize {
    let logical = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    logical.saturating_sub(1).max(1).min(6)
}

fn clip_worker_count() -> usize {
    background_worker_count().min(4)
}
'''
if marker not in text:
    raise SystemExit('constant marker missing')
text = text.replace(marker, insert, 1)

start = text.index('    let total = candidates.len();')
end = text.index('    let mut removed = 0usize;', start)
new_scan = '''    let total = candidates.len();
    let mut seen_by_root: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let mut pending = Vec::<PendingImage>::new();

    // Keep filesystem/SQLite state checks cheap and serialized, but move image
    // decoding + metadata extraction to a bounded worker pool below.
    for (index, (root, path)) in candidates.iter().enumerate() {
        seen_by_root
            .entry(root.clone())
            .or_default()
            .push(path.clone());

        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = tx.send(WorkerMessage::Error(format!(
                    "Cannot read {}: {err}",
                    path.display()
                )));
                continue;
            }
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);

        let unchanged = db::existing_file_state(&conn, path)?
            .map(|(old_size, old_modified, _)| old_size == size && old_modified == modified)
            .unwrap_or(false);

        if !unchanged {
            pending.push(PendingImage {
                root: root.clone(),
                path: path.clone(),
                size,
                modified,
            });
        }

        if index % 32 == 0 || index + 1 == total {
            let _ = tx.send(WorkerMessage::Progress {
                done: index + 1,
                total,
            });
        }
    }

    let changed_total = pending.len();
    let _ = tx.send(WorkerMessage::Status(format!(
        "Preparing {changed_total} changed image{} on {} background worker{}…",
        if changed_total == 1 { "" } else { "s" },
        background_worker_count(),
        if background_worker_count() == 1 { "" } else { "s" }
    )));

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(background_worker_count())
        .thread_name(|index| format!("image-index-{index}"))
        .build()
        .context("creating image indexing worker pool")?;
    let prepared_count = AtomicUsize::new(0);

    let prepared: Vec<PreparedImage> = pool.install(|| {
        pending
            .par_iter()
            .filter_map(|item| {
                let result = inspect_image(&item.path).map(
                    |(width, height, dominant, visual_hash, color_histogram)| {
                        let text = metadata::extract(&item.path);
                        PreparedImage {
                            root: item.root.clone(),
                            path: item.path.clone(),
                            file_name: item
                                .path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or_default()
                                .to_owned(),
                            extension: item
                                .path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .unwrap_or_default()
                                .to_ascii_lowercase(),
                            size: item.size,
                            modified: item.modified,
                            width,
                            height,
                            description: text.description,
                            keywords: text.keywords,
                            dominant,
                            visual_hash,
                            color_histogram,
                        }
                    },
                );

                let done = prepared_count.fetch_add(1, Ordering::Relaxed) + 1;
                if done % 16 == 0 || done == changed_total {
                    let _ = tx.send(WorkerMessage::Status(format!(
                        "Decoding/reading metadata: {done}/{changed_total}"
                    )));
                }

                match result {
                    Ok(value) => Some(value),
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Cannot decode {}: {err:#}",
                            item.path.display()
                        )));
                        None
                    }
                }
            })
            .collect()
    });

    // SQLite writes stay on this worker thread; CPU-heavy preparation above is
    // parallelized without sharing a Connection across threads.
    for item in &prepared {
        db::upsert_image(
            &conn,
            &item.path,
            &item.root,
            &item.file_name,
            &item.extension,
            item.size,
            item.modified,
            item.width,
            item.height,
            &item.description,
            &item.keywords,
            item.dominant,
            item.visual_hash,
            &item.color_histogram,
        )?;
    }
    let changed = prepared.len();

'''
text = text[:start] + new_scan + text[end:]

start = text.index('fn build_visual_descriptors(')
end = text.index('\nfn build_embeddings(', start)
new_visual = '''fn build_visual_descriptors(
    conn: &rusqlite::Connection,
    paths: &[PathBuf],
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(background_worker_count())
        .thread_name(|index| format!("visual-index-{index}"))
        .build()
        .context("creating visual descriptor worker pool")?;
    let done = AtomicUsize::new(0);

    let descriptors: Vec<(PathBuf, u64, Vec<f32>)> = pool.install(|| {
        paths
            .par_iter()
            .filter_map(|path| {
                let result = decode_image(path).map(|image| visual_descriptor(&image));
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current % 25 == 0 || current == total {
                    let _ = tx.send(WorkerMessage::Status(format!(
                        "Building texture/color index: {current}/{total}"
                    )));
                }
                match result {
                    Ok((_, visual_hash, color_histogram)) => {
                        Some((path.clone(), visual_hash, color_histogram))
                    }
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Cannot build visual descriptor for {}: {err:#}",
                            path.display()
                        )));
                        None
                    }
                }
            })
            .collect()
    });

    for (path, visual_hash, color_histogram) in descriptors {
        db::set_visual_descriptor(conn, &path, visual_hash, &color_histogram)?;
    }
    Ok(())
}
'''
text = text[:start] + new_visual + text[end:]

text = text.replace('.with_intra_threads(4);', '.with_intra_threads(clip_worker_count());')

path.write_text(text, encoding='utf-8')
