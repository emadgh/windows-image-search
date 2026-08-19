use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::time::UNIX_EPOCH;

const CACHE_EDGE: u32 = 512;

#[derive(Debug)]
pub enum ThumbnailResult {
    Ready {
        path: PathBuf,
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Failed {
        path: PathBuf,
    },
}

pub struct ThumbnailPool {
    cache_dir: PathBuf,
    job_tx: Sender<PathBuf>,
    result_rx: Receiver<ThumbnailResult>,
    pending: HashSet<PathBuf>,
}

impl ThumbnailPool {
    pub fn new(cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let (job_tx, job_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, result_rx) = mpsc::channel::<ThumbnailResult>();
        let shared_rx = Arc::new(Mutex::new(job_rx));

        let logical = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4);
        let workers = logical.saturating_sub(1).clamp(2, 4);

        for _ in 0..workers {
            let rx = Arc::clone(&shared_rx);
            let tx = result_tx.clone();
            let cache = cache_dir.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let Ok(lock) = rx.lock() else {
                        break;
                    };
                    match lock.recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    }
                };

                let result = match load_or_build(&cache, &job) {
                    Some((width, height, rgba)) => ThumbnailResult::Ready {
                        path: job,
                        width,
                        height,
                        rgba,
                    },
                    None => ThumbnailResult::Failed { path: job },
                };
                let _ = tx.send(result);
            });
        }

        Self {
            cache_dir,
            job_tx,
            result_rx,
            pending: HashSet::new(),
        }
    }

    pub fn request(&mut self, path: &Path) {
        if self.pending.insert(path.to_path_buf()) {
            let _ = self.job_tx.send(path.to_path_buf());
        }
    }

    pub fn try_recv(&mut self) -> Option<ThumbnailResult> {
        match self.result_rx.try_recv().ok() {
            Some(result) => {
                let path = match &result {
                    ThumbnailResult::Ready { path, .. } => path,
                    ThumbnailResult::Failed { path } => path,
                };
                self.pending.remove(path);
                Some(result)
            }
            None => None,
        }
    }

    pub fn clear_cache(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cache_dir);
        let _ = std::fs::create_dir_all(&self.cache_dir);
        self.pending.clear();
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

fn load_or_build(cache_dir: &Path, source: &Path) -> Option<(usize, usize, Vec<u8>)> {
    let cache_path = thumbnail_cache_path(cache_dir, source);

    if cache_path.exists() {
        if let Ok(reader) = image::ImageReader::open(&cache_path) {
            if let Ok(reader) = reader.with_guessed_format() {
                if let Ok(image) = reader.decode() {
                    return Some(to_rgba(image));
                }
            }
        }
        let _ = std::fs::remove_file(&cache_path);
    }

    let image = image::ImageReader::open(source)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = image.thumbnail(CACHE_EDGE, CACHE_EDGE).to_rgb8();

    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = File::create(&cache_path) {
        let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), 84);
        let _ = encoder.encode_image(&DynamicImage::ImageRgb8(thumb.clone()));
    }

    Some(to_rgba(DynamicImage::ImageRgb8(thumb)))
}

fn to_rgba(image: DynamicImage) -> (usize, usize, Vec<u8>) {
    let rgba = image.to_rgba8();
    (
        rgba.width() as usize,
        rgba.height() as usize,
        rgba.into_raw(),
    )
}

fn thumbnail_cache_path(cache_dir: &Path, source: &Path) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    source.to_string_lossy().hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(source) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(delta) = modified.duration_since(UNIX_EPOCH) {
                delta.as_secs().hash(&mut hasher);
                delta.subsec_nanos().hash(&mut hasher);
            }
        }
    }
    cache_dir.join(format!("{:016x}.jpg", hasher.finish()))
}
