//! Cooperative cancellation and UI acceptance of asynchronous search results.
use anyhow::{bail, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub id: u64,
    cancelled: Arc<AtomicBool>,
}

impl SearchRequest {
    pub fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            bail!("Search cancelled");
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct SearchSession {
    generation: u64,
    current: Option<SearchRequest>,
}

impl SearchSession {
    pub fn start(&mut self) -> SearchRequest {
        self.cancel();
        self.generation = self
            .generation
            .checked_add(1)
            .expect("search generation exhausted");
        let request = SearchRequest {
            id: self.generation,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        self.current = Some(request.clone());
        request
    }

    pub fn accepts(&self, id: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|request| request.id == id)
    }

    pub fn cancel(&mut self) {
        if let Some(request) = self.current.take() {
            request.cancelled.store(true, Ordering::Relaxed);
        }
    }

    pub fn finish(&mut self, id: u64) -> bool {
        if !self.accepts(id) {
            return false;
        }
        self.current = None;
        true
    }
}

impl Drop for SearchSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_completion_cannot_finish_replacement_request() {
        let mut session = SearchSession::default();
        let first = session.start();
        let second = session.start();
        assert!(first.check().is_err());
        assert!(!session.finish(first.id));
        assert!(session.accepts(second.id));
        assert!(second.check().is_ok());
        assert!(session.finish(second.id));
        assert!(!session.accepts(second.id));
    }

    #[test]
    fn cancel_rejects_already_queued_messages_and_notifies_worker() {
        let mut session = SearchSession::default();
        let request = session.start();
        let worker = request.clone();
        session.cancel();
        session.cancel();
        assert!(!session.accepts(request.id));
        assert!(worker.check().is_err());
    }

    #[test]
    fn dropping_session_cancels_outstanding_work() {
        let request = {
            let mut session = SearchSession::default();
            session.start()
        };
        assert!(request.check().is_err());
    }
}
