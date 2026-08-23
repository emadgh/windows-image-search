use crate::{db, people_filter};
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
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
                        Ok(conn) => search_text_and_people(&db_path, conn, &request.query)
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

fn search_text_and_people(
    db_path: &Path,
    conn: &Connection,
    query: &str,
) -> Result<HashSet<PathBuf>> {
    let tokens = query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Ok(HashSet::new());
    }

    // People metadata is resolved on the background search worker, never from the
    // egui frame loop. The root registry may change through Collections, so read it
    // for each dispatched query rather than capturing a startup-only snapshot.
    let roots = db::load_roots(db_path)?;
    let named_people = people_filter::load_named_people(db_path)?;
    let mut token_sets = Vec::with_capacity(tokens.len());

    for token in tokens {
        let mut matches = db::search_text(conn, token)?
            .into_iter()
            .collect::<HashSet<_>>();

        let needle = token.to_lowercase();
        let matching_people = named_people
            .iter()
            .filter(|person| person.display_name.to_lowercase().contains(&needle))
            .map(|person| person.person_id.clone())
            .collect::<Vec<_>>();
        if !matching_people.is_empty() {
            let resolved = people_filter::resolve_filter(
                db_path,
                &roots,
                &matching_people,
                people_filter::PeopleFilterMode::Any,
            )?;
            matches.extend(resolved.matching_images);
        }
        token_sets.push(matches);
    }

    Ok(intersect_token_sets(token_sets))
}

fn intersect_token_sets(mut sets: Vec<HashSet<PathBuf>>) -> HashSet<PathBuf> {
    let Some(mut result) = sets.pop() else {
        return HashSet::new();
    };
    for set in sets {
        result.retain(|path| set.contains(path));
        if result.is_empty() {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> HashSet<PathBuf> {
        values.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn mixed_text_and_person_tokens_intersect() {
        let metadata_or_person_for_first = paths(&["alice-beach.jpg", "alice-city.jpg"]);
        let metadata_or_person_for_second = paths(&["alice-beach.jpg", "other-beach.jpg"]);
        assert_eq!(
            intersect_token_sets(vec![
                metadata_or_person_for_first,
                metadata_or_person_for_second,
            ]),
            paths(&["alice-beach.jpg"])
        );
    }

    #[test]
    fn empty_token_set_list_has_no_matches() {
        assert!(intersect_token_sets(Vec::new()).is_empty());
    }
}
