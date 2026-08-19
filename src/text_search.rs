use crate::db;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

#[derive(Debug)]
pub struct TextSearchResult {
    pub generation: u64,
    pub query: String,
    pub paths: Result<HashSet<PathBuf>, String>,
    pub elapsed_ms: u128,
}

struct SearchRequest {
    generation: u64,
    query: String,
}

pub struct TextSearchService {
    request_tx: Sender<SearchRequest>,
    result_rx: Receiver<TextSearchResult>,
}

impl TextSearchService {
    pub fn new(db_path: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<SearchRequest>();
        let (result_tx, result_rx) = mpsc::channel::<TextSearchResult>();

        std::thread::Builder::new()
            .name("text-search-service".to_owned())
            .spawn(move || {
                let connection = db::open(&db_path).map_err(|err| format!("{err:#}"));
                while let Ok(mut request) = request_rx.recv() {
                    // If the user typed several characters while a previous query
                    // was queued, only execute the newest request.
                    while let Ok(newer) = request_rx.try_recv() {
                        request = newer;
                    }

                    let started = Instant::now();
                    let paths = match &connection {
                        Ok(conn) => db::search_text(conn, &request.query)
                            .map(|paths| paths.into_iter().collect())
                            .map_err(|err| format!("{err:#}")),
                        Err(err) => Err(err.clone()),
                    };
                    let _ = result_tx.send(TextSearchResult {
                        generation: request.generation,
                        query: request.query,
                        paths,
                        elapsed_ms: started.elapsed().as_millis(),
                    });
                }
            })
            .expect("creating text search worker");

        Self {
            request_tx,
            result_rx,
        }
    }

    pub fn request(&self, generation: u64, query: String) {
        let _ = self.request_tx.send(SearchRequest { generation, query });
    }

    pub fn try_recv(&self) -> Option<TextSearchResult> {
        self.result_rx.try_recv().ok()
    }
}
