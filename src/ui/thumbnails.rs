use crate::{portable, thumbnail_cache};
use image::DynamicImage;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver},
    Arc, Condvar, Mutex, RwLock,
};

// A request is considered stale after enough newer visible-thumbnail requests
// have arrived. The value is intentionally larger than a very dense 4K grid,
// so thumbnails still visible in the current viewport are not discarded while
// fast scrolling quickly invalidates older viewport work.
const STALE_REQUEST_DISTANCE: u64 = 1_024;

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThumbnailJob {
    path: PathBuf,
    request_sequence: u64,
    revision: u64,
}

impl Ord for ThumbnailJob {
    fn cmp(&self, other: &Self) -> Ordering {
        self.request_sequence
            .cmp(&other.request_sequence)
            .then_with(|| self.revision.cmp(&other.revision))
    }
}

impl PartialOrd for ThumbnailJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingRequest {
    last_seen_sequence: u64,
    queued_sequence: u64,
    revision: u64,
    in_flight: bool,
    completed: bool,
}

#[derive(Default)]
struct SchedulerState {
    latest_sequence: u64,
    queue: BinaryHeap<ThumbnailJob>,
    pending: HashMap<PathBuf, PendingRequest>,
}

impl SchedulerState {
    fn request(&mut self, path: &Path) -> bool {
        self.latest_sequence = self.latest_sequence.saturating_add(1);
        let sequence = self.latest_sequence;
        let mut enqueue = None;

        if let Some(pending) = self.pending.get_mut(path) {
            let was_stale =
                sequence.saturating_sub(pending.last_seen_sequence) > STALE_REQUEST_DISTANCE;
            pending.last_seen_sequence = sequence;
            if was_stale && !pending.in_flight && !pending.completed {
                pending.revision = pending.revision.saturating_add(1);
                pending.queued_sequence = sequence;
                enqueue = Some(ThumbnailJob {
                    path: path.to_path_buf(),
                    request_sequence: sequence,
                    revision: pending.revision,
                });
            }
        } else {
            let pending = PendingRequest {
                last_seen_sequence: sequence,
                queued_sequence: sequence,
                revision: 1,
                in_flight: false,
                completed: false,
            };
            self.pending.insert(path.to_path_buf(), pending);
            enqueue = Some(ThumbnailJob {
                path: path.to_path_buf(),
                request_sequence: sequence,
                revision: 1,
            });
        }

        if let Some(job) = enqueue {
            self.queue.push(job);
            true
        } else {
            false
        }
    }

    fn pop_next(&mut self) -> Option<ThumbnailJob> {
        while let Some(job) = self.queue.pop() {
            let Some(pending) = self.pending.get(&job.path).copied() else {
                continue;
            };

            if pending.revision != job.revision
                || pending.in_flight
                || pending.completed
                || pending.queued_sequence != job.request_sequence
            {
                continue;
            }

            let stale = self
                .latest_sequence
                .saturating_sub(pending.last_seen_sequence)
                > STALE_REQUEST_DISTANCE;
            if stale {
                self.pending.remove(&job.path);
                continue;
            }

            if let Some(pending) = self.pending.get_mut(&job.path) {
                pending.in_flight = true;
            }
            return Some(job);
        }
        None
    }

    fn finish(&mut self, job: &ThumbnailJob) {
        if let Some(pending) = self.pending.get_mut(&job.path) {
            if pending.revision == job.revision {
                pending.in_flight = false;
                pending.completed = true;
            }
        }
    }

    fn consume_result(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    fn clear(&mut self) {
        self.queue.clear();
        self.pending.clear();
    }
}

pub struct ThumbnailPool {
    fallback_cache_dir: PathBuf,
    roots: Arc<RwLock<Vec<PathBuf>>>,
    scheduler: Arc<(Mutex<SchedulerState>, Condvar)>,
    result_rx: Receiver<ThumbnailResult>,
}

impl ThumbnailPool {
    pub fn new(fallback_cache_dir: PathBuf, roots: Vec<PathBuf>) -> Self {
        let _ = std::fs::create_dir_all(&fallback_cache_dir);
        let (result_tx, result_rx) = mpsc::channel::<ThumbnailResult>();
        let scheduler = Arc::new((Mutex::new(SchedulerState::default()), Condvar::new()));
        let roots = Arc::new(RwLock::new(roots));

        let logical = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4);
        let workers = logical.saturating_sub(1).clamp(2, 4);

        for _ in 0..workers {
            let scheduler = Arc::clone(&scheduler);
            let roots = Arc::clone(&roots);
            let tx = result_tx.clone();
            let fallback_cache = fallback_cache_dir.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let (lock, wake) = &*scheduler;
                    let mut state = match lock.lock() {
                        Ok(state) => state,
                        Err(_) => break,
                    };
                    loop {
                        if let Some(job) = state.pop_next() {
                            break job;
                        }
                        state = match wake.wait(state) {
                            Ok(state) => state,
                            Err(_) => return,
                        };
                    }
                };

                let result = match load_or_build(&fallback_cache, &roots, &job.path) {
                    Some((width, height, rgba)) => ThumbnailResult::Ready {
                        path: job.path.clone(),
                        width,
                        height,
                        rgba,
                    },
                    None => ThumbnailResult::Failed {
                        path: job.path.clone(),
                    },
                };

                {
                    let (lock, _) = &*scheduler;
                    if let Ok(mut state) = lock.lock() {
                        state.finish(&job);
                    }
                }
                let _ = tx.send(result);
            });
        }

        Self {
            fallback_cache_dir,
            roots,
            scheduler,
            result_rx,
        }
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        if let Ok(mut current) = self.roots.write() {
            *current = roots;
        }
    }

    pub fn request(&mut self, path: &Path) {
        let (lock, wake) = &*self.scheduler;
        if let Ok(mut state) = lock.lock() {
            if state.request(path) {
                wake.notify_one();
            }
        }
    }

    pub fn try_recv(&mut self) -> Option<ThumbnailResult> {
        let result = self.result_rx.try_recv().ok()?;
        let path = match &result {
            ThumbnailResult::Ready { path, .. } => path,
            ThumbnailResult::Failed { path } => path,
        };
        let (lock, _) = &*self.scheduler;
        if let Ok(mut state) = lock.lock() {
            state.consume_result(path);
        }
        Some(result)
    }

    pub fn clear_cache(&mut self) {
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
    let root = roots
        .read()
        .ok()
        .and_then(|roots| portable::indexed_root_for_path(source, &roots).cloned());
    let image = match root {
        Some(root) if portable::is_indexed_root(&root) => {
            thumbnail_cache::load_or_build_for_root(&root, source)
        }
        _ => thumbnail_cache::load_or_build(fallback_cache, source),
    }?;
    Some(to_rgba(image))
}

fn to_rgba(image: DynamicImage) -> (usize, usize, Vec<u8>) {
    let rgba = image.to_rgba8();
    (
        rgba.width() as usize,
        rgba.height() as usize,
        rgba.into_raw(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_viewport_request_is_popped_first() {
        let mut state = SchedulerState::default();
        state.request(Path::new("old-a.jpg"));
        state.request(Path::new("old-b.jpg"));
        state.request(Path::new("current.jpg"));

        let job = state.pop_next().expect("a queued thumbnail");
        assert_eq!(job.path, PathBuf::from("current.jpg"));
    }

    #[test]
    fn stale_offscreen_work_is_skipped_before_decode() {
        let mut state = SchedulerState::default();
        let stale = PathBuf::from("stale.jpg");
        state.request(&stale);

        for index in 0..=STALE_REQUEST_DISTANCE {
            state.request(Path::new(&format!("visible-{index}.jpg")));
        }

        while let Some(job) = state.pop_next() {
            if job.path == stale {
                panic!("stale thumbnail should have been cancelled before decode");
            }
            state.finish(&job);
            state.consume_result(&job.path);
        }
        assert!(!state.pending.contains_key(&stale));
    }

    #[test]
    fn duplicate_visible_requests_do_not_duplicate_queue_entries() {
        let mut state = SchedulerState::default();
        let path = Path::new("same.jpg");
        assert!(state.request(path));
        for _ in 0..100 {
            assert!(!state.request(path));
        }
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.pending.len(), 1);
    }

    #[test]
    fn returning_stale_path_is_repromoted_instead_of_waiting_old_turn() {
        let mut state = SchedulerState::default();
        let returning = PathBuf::from("returning.jpg");
        state.request(&returning);

        for index in 0..=STALE_REQUEST_DISTANCE {
            state.request(Path::new(&format!("other-{index}.jpg")));
        }
        assert!(state.request(&returning));

        let job = state.pop_next().expect("returning path should be promoted");
        assert_eq!(job.path, returning);
    }
}
