use crate::portable;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const EVENT_DEBOUNCE: Duration = Duration::from_millis(420);

#[derive(Debug)]
pub enum FsWatchMessage {
    PathsChanged(Vec<PathBuf>),
    ReconcileRequired(String),
    Status(String),
}

enum ControlMessage {
    SetRoots(Vec<PathBuf>),
}

pub struct FsWatchService {
    control_tx: Sender<ControlMessage>,
    result_rx: Receiver<FsWatchMessage>,
}

impl FsWatchService {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let (control_tx, control_rx) = mpsc::channel::<ControlMessage>();
        let (result_tx, result_rx) = mpsc::channel::<FsWatchMessage>();

        std::thread::Builder::new()
            .name("filesystem-watch-service".to_owned())
            .spawn(move || run_watcher(roots, control_rx, result_tx))
            .expect("creating filesystem watcher worker");

        Self {
            control_tx,
            result_rx,
        }
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        let _ = self.control_tx.send(ControlMessage::SetRoots(roots));
    }

    pub fn try_recv(&self) -> Option<FsWatchMessage> {
        self.result_rx.try_recv().ok()
    }
}

fn run_watcher(
    initial_roots: Vec<PathBuf>,
    control_rx: Receiver<ControlMessage>,
    result_tx: Sender<FsWatchMessage>,
) {
    let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match notify::recommended_watcher(event_tx) {
        Ok(watcher) => watcher,
        Err(err) => {
            let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                "Live filesystem watcher could not start: {err}"
            )));
            return;
        }
    };

    let mut watched_roots = Vec::<PathBuf>::new();
    replace_roots(&mut watcher, &mut watched_roots, initial_roots, &result_tx);

    let mut pending_paths = HashSet::<PathBuf>::new();
    let mut flush_at: Option<Instant> = None;

    loop {
        while let Ok(control) = control_rx.try_recv() {
            match control {
                ControlMessage::SetRoots(roots) => {
                    replace_roots(&mut watcher, &mut watched_roots, roots, &result_tx)
                }
            }
        }

        let timeout = flush_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(100))
            .min(Duration::from_millis(100));

        match event_rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                if event_kind_needs_indexing(&event.kind) {
                    pending_paths.extend(event.paths.into_iter().filter(|path| {
                        !watched_roots
                            .iter()
                            .any(|root| portable::is_internal_path(root, path))
                    }));
                    if !pending_paths.is_empty() {
                        flush_at = Some(Instant::now() + EVENT_DEBOUNCE);
                    }
                }
            }
            Ok(Err(err)) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                    "Filesystem watcher reported an error; run Rescan to reconcile the index: {err}"
                )));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(
                    "Filesystem watcher event channel stopped; run Rescan to reconcile the index"
                        .to_owned(),
                ));
                return;
            }
        }

        if flush_at.is_some_and(|deadline| Instant::now() >= deadline) {
            flush_at = None;
            if !pending_paths.is_empty() {
                let mut paths: Vec<PathBuf> = pending_paths.drain().collect();
                paths.sort();
                let _ = result_tx.send(FsWatchMessage::PathsChanged(paths));
            }
        }
    }
}

fn replace_roots(
    watcher: &mut notify::RecommendedWatcher,
    watched_roots: &mut Vec<PathBuf>,
    new_roots: Vec<PathBuf>,
    result_tx: &Sender<FsWatchMessage>,
) {
    for root in watched_roots.drain(..) {
        let _ = watcher.unwatch(&root);
    }

    let mut active = Vec::new();
    for root in new_roots {
        if !root.exists() {
            let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                "Cannot live-watch missing indexed root: {}",
                root.display()
            )));
            continue;
        }
        match watcher.watch(&root, RecursiveMode::Recursive) {
            Ok(()) => active.push(root),
            Err(err) => {
                let _ = result_tx.send(FsWatchMessage::ReconcileRequired(format!(
                    "Cannot live-watch {}: {err}",
                    root.display()
                )));
            }
        }
    }

    *watched_roots = active;
    let _ = result_tx.send(FsWatchMessage::Status(format!(
        "Live filesystem watching: {} indexed root{}",
        watched_roots.len(),
        if watched_roots.len() == 1 { "" } else { "s" }
    )));
}

fn event_kind_needs_indexing(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};

    #[test]
    fn access_events_are_ignored_but_content_changes_are_kept() {
        assert!(!event_kind_needs_indexing(&EventKind::Access(
            AccessKind::Any
        )));
        assert!(event_kind_needs_indexing(&EventKind::Create(
            CreateKind::Any
        )));
        assert!(event_kind_needs_indexing(&EventKind::Modify(
            ModifyKind::Any
        )));
        assert!(event_kind_needs_indexing(&EventKind::Remove(
            RemoveKind::Any
        )));
    }
}
