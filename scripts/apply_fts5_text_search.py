from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# db.rs: FTS5 external-content index + triggers and search functions.
# -----------------------------------------------------------------------------
path = Path("src/db.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use rusqlite::{params, Connection};\n",
    "use rusqlite::{params, params_from_iter, Connection};\n",
    "rusqlite dynamic params import",
)

text = replace_once(
    text,
    '''    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_images_root_scan ON images(root, last_seen_scan)",
        [],
    )?;

    Ok(conn)
''',
    '''    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_images_root_scan ON images(root, last_seen_scan)",
        [],
    )?;
    ensure_text_search_index(&conn)?;

    Ok(conn)
''',
    "initialize FTS search index",
)

insert_before = "fn ensure_column(conn: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {\n"
fts_helpers = r'''fn ensure_text_search_index(conn: &Connection) -> Result<()> {
    let existed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'images_fts')",
        [],
        |row| row.get(0),
    )?;

    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS images_fts USING fts5(
            file_name,
            path,
            description,
            keywords,
            content='images',
            content_rowid='rowid',
            tokenize='trigram'
        );

        CREATE TRIGGER IF NOT EXISTS images_fts_ai AFTER INSERT ON images BEGIN
            INSERT INTO images_fts(rowid, file_name, path, description, keywords)
            VALUES (new.rowid, new.file_name, new.path, new.description, new.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS images_fts_ad AFTER DELETE ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, file_name, path, description, keywords)
            VALUES ('delete', old.rowid, old.file_name, old.path, old.description, old.keywords);
        END;

        CREATE TRIGGER IF NOT EXISTS images_fts_au
        AFTER UPDATE OF file_name, path, description, keywords ON images BEGIN
            INSERT INTO images_fts(images_fts, rowid, file_name, path, description, keywords)
            VALUES ('delete', old.rowid, old.file_name, old.path, old.description, old.keywords);
            INSERT INTO images_fts(rowid, file_name, path, description, keywords)
            VALUES (new.rowid, new.file_name, new.path, new.description, new.keywords);
        END;
        "#,
    )?;

    if !existed {
        conn.execute("INSERT INTO images_fts(images_fts) VALUES('rebuild')", [])?;
    }
    Ok(())
}

fn fts_phrase(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

fn like_pattern(token: &str) -> String {
    let escaped = token
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub fn search_text(conn: &Connection, query: &str) -> Result<Vec<PathBuf>> {
    let tokens: Vec<&str> = query.split_whitespace().filter(|token| !token.is_empty()).collect();
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    if tokens.iter().all(|token| token.chars().count() >= 3) {
        let expression = tokens
            .iter()
            .map(|token| fts_phrase(token))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut stmt = conn.prepare(
            "SELECT images.path FROM images_fts JOIN images ON images.rowid = images_fts.rowid WHERE images_fts MATCH ?1 ORDER BY bm25(images_fts)",
        )?;
        let rows = stmt.query_map(params![expression], |row| row.get::<_, String>(0))?;
        return Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect());
    }

    // FTS5's trigram tokenizer cannot satisfy one- and two-character substring
    // queries. Preserve the old contains semantics with a parameterized LIKE
    // fallback on this background search connection.
    let clause = "(file_name LIKE ? ESCAPE '\\' COLLATE NOCASE OR path LIKE ? ESCAPE '\\' COLLATE NOCASE OR description LIKE ? ESCAPE '\\' COLLATE NOCASE OR keywords LIKE ? ESCAPE '\\' COLLATE NOCASE)";
    let sql = format!(
        "SELECT path FROM images WHERE {} ORDER BY file_name COLLATE NOCASE",
        std::iter::repeat_n(clause, tokens.len()).collect::<Vec<_>>().join(" AND ")
    );
    let mut values = Vec::<String>::with_capacity(tokens.len() * 4);
    for token in tokens {
        let pattern = like_pattern(token);
        for _ in 0..4 {
            values.push(pattern.clone());
        }
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |row| row.get::<_, String>(0))?;
    Ok(rows.filter_map(|row| row.ok()).map(PathBuf::from).collect())
}

'''
text = replace_once(text, insert_before, fts_helpers + insert_before, "FTS helper insertion")

# Add regression tests before the first existing test.
test_marker = '''    #[test]
    fn scan_generation_prunes_stale_rows_only_after_explicit_cleanup() {
'''
test_block = r'''    #[test]
    fn fts_text_search_supports_substrings_and_and_semantics() {
        let db_path = temp_db_path("fts-text-search");
        let root = std::env::temp_dir().join("windows-image-search-fts-root");
        let brown = root.join("BrownMarble_A01.jpg");
        let gray = root.join("SilverCement_B02.jpg");

        {
            let conn = open(&db_path).unwrap();
            upsert_image(
                &conn,
                &brown,
                &root,
                "BrownMarble_A01.jpg",
                "jpg",
                1,
                1,
                32,
                32,
                "warm stone with gold veins",
                "brown marble polished",
                [120, 70, 40],
                1,
                &[1.0],
            )
            .unwrap();
            upsert_image(
                &conn,
                &gray,
                &root,
                "SilverCement_B02.jpg",
                "jpg",
                1,
                1,
                32,
                32,
                "cool concrete texture",
                "gray cement",
                [130, 130, 130],
                2,
                &[1.0],
            )
            .unwrap();

            let substring = search_text(&conn, "marb").unwrap();
            assert_eq!(substring, vec![brown.clone()]);

            let and_query = search_text(&conn, "brown vein").unwrap();
            assert_eq!(and_query, vec![brown.clone()]);

            let short_fallback = search_text(&conn, "A0").unwrap();
            assert_eq!(short_fallback, vec![brown.clone()]);

            // Updating searchable metadata must refresh FTS, while embedding-only
            // updates do not need to touch the FTS index.
            upsert_image(
                &conn,
                &brown,
                &root,
                "BrownMarble_A01.jpg",
                "jpg",
                2,
                2,
                32,
                32,
                "warm stone without the previous metallic term",
                "brown marble polished",
                [120, 70, 40],
                1,
                &[1.0],
            )
            .unwrap();
            assert!(search_text(&conn, "gold").unwrap().is_empty());
        }

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn scan_generation_prunes_stale_rows_only_after_explicit_cleanup() {
'''
text = replace_once(text, test_marker, test_block, "FTS regression tests")
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# New text_search.rs service: persistent read connection, request coalescing,
# result generation IDs, and no work on the eframe update thread.
# -----------------------------------------------------------------------------
Path("src/text_search.rs").write_text(
    r'''use crate::db;
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
''',
    encoding="utf-8",
)


# -----------------------------------------------------------------------------
# main.rs
# -----------------------------------------------------------------------------
path = Path("src/main.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "mod settings;\nmod ui;\n",
    "mod settings;\nmod text_search;\nmod ui;\n",
    "text search module",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/mod.rs
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::settings::{self, IndexingSettings};\n",
    "use crate::settings::{self, IndexingSettings};\nuse crate::text_search::TextSearchService;\n",
    "UI text search import",
)
text = replace_once(
    text,
    "use std::time::Duration;\n",
    "use std::time::{Duration, Instant};\n",
    "UI Instant import",
)
text = replace_once(
    text,
    '''    pub(super) search_text: String,
    pub(super) color_enabled: bool,
''',
    '''    pub(super) search_text: String,
    text_search_service: TextSearchService,
    text_search_matches: Option<HashSet<PathBuf>>,
    text_search_observed: String,
    text_search_due: Option<Instant>,
    text_search_generation: u64,
    text_search_pending: bool,
    pub(super) color_enabled: bool,
''',
    "UI text search fields",
)
text = replace_once(
    text,
    '''        let embedding_service = EmbeddingService::new(model_cache);
        let images = db::load_image_summaries(&db_path).unwrap_or_default();
''',
    '''        let embedding_service = EmbeddingService::new(model_cache);
        let text_search_service = TextSearchService::new(db_path.clone());
        let images = db::load_image_summaries(&db_path).unwrap_or_default();
''',
    "create text search service",
)
text = replace_once(
    text,
    '''            search_text: String::new(),
            color_enabled: false,
''',
    '''            search_text: String::new(),
            text_search_service,
            text_search_matches: None,
            text_search_observed: String::new(),
            text_search_due: None,
            text_search_generation: 0,
            text_search_pending: false,
            color_enabled: false,
''',
    "initialize text search state",
)

# Refresh active text results as live indexing commits new searchable records.
text = replace_once(
    text,
    '''                WorkerMessage::IndexedBatch(records) => self.merge_indexed_batch(records),
                WorkerMessage::Reload => {
''',
    '''                WorkerMessage::IndexedBatch(records) => {
                    self.merge_indexed_batch(records);
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::Reload => {
''',
    "refresh text search on live batches",
)
text = replace_once(
    text,
    '''                    self.progress = None;
                }
                WorkerMessage::SimilarityResults(results) => {
''',
    '''                    self.progress = None;
                    self.refresh_text_search_after_data_change();
                }
                WorkerMessage::SimilarityResults(results) => {
''',
    "refresh text search on reload",
)

# Replace per-frame haystack allocation with cached path membership.
old_visible = '''    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let tokens: Vec<String> = self
            .search_text
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        self.source()
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if !tokens.is_empty() {
                    let haystack = format!(
                        "{} {} {} {}",
                        record.file_name,
                        record.path.display(),
                        record.description,
                        record.keywords
                    )
                    .to_ascii_lowercase();
                    if !tokens.iter().all(|token| haystack.contains(token)) {
                        return false;
                    }
                }
                !self.color_enabled
                    || views::color_distance(record.dominant, self.target_color)
                        <= self.color_tolerance
            })
            .map(|(index, _)| index)
            .collect()
    }
'''
new_visible = '''    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let text_filter_active = !self.search_text.trim().is_empty();
        self.source()
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                if text_filter_active {
                    let Some(matches) = &self.text_search_matches else {
                        return false;
                    };
                    if !matches.contains(&record.path) {
                        return false;
                    }
                }
                !self.color_enabled
                    || views::color_distance(record.dominant, self.target_color)
                        <= self.color_tolerance
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn observe_text_search_input(&mut self) {
        if self.search_text == self.text_search_observed {
            return;
        }
        self.text_search_observed = self.search_text.clone();
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_matches = None;

        if self.search_text.trim().is_empty() {
            self.text_search_due = None;
            self.text_search_pending = false;
        } else {
            self.text_search_due = Some(Instant::now() + Duration::from_millis(160));
            self.text_search_pending = true;
        }
    }

    fn refresh_text_search_after_data_change(&mut self) {
        if self.search_text.trim().is_empty() {
            return;
        }
        self.text_search_generation = self.text_search_generation.wrapping_add(1);
        self.text_search_due = Some(Instant::now() + Duration::from_millis(220));
        self.text_search_pending = true;
    }

    fn dispatch_text_search_if_due(&mut self) {
        let Some(due) = self.text_search_due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.text_search_due = None;
        self.text_search_service
            .request(self.text_search_generation, self.search_text.clone());
    }

    fn process_text_search_results(&mut self) {
        while let Some(result) = self.text_search_service.try_recv() {
            if result.generation != self.text_search_generation || result.query != self.search_text {
                continue;
            }
            match result.paths {
                Ok(paths) => {
                    let count = paths.len();
                    self.text_search_matches = Some(paths);
                    self.text_search_pending = false;
                    self.status = format!(
                        "Indexed text search: {count} match{} in {} ms",
                        if count == 1 { "" } else { "es" },
                        result.elapsed_ms
                    );
                }
                Err(err) => {
                    self.text_search_matches = Some(HashSet::new());
                    self.text_search_pending = false;
                    self.last_error = Some(format!("Text search failed: {err}"));
                }
            }
        }
    }
'''
text = replace_once(text, old_visible, new_visible, "replace per-frame text filtering")

# Search box: capture response and show pending state.
text = replace_once(
    text,
    '''                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .hint_text("filename, path, description, keywords…")
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(8.0);
''',
    '''                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .hint_text("filename, path, description, keywords…")
                            .desired_width(f32::INFINITY),
                    );
                    if self.text_search_pending {
                        ui.small("Searching indexed text…");
                    }

                    ui.add_space(8.0);
''',
    "text search pending UI",
)

# Update loop must observe, dispatch and receive before rendering.
text = replace_once(
    text,
    '''        self.process_worker_messages();
        self.process_thumbnail_messages(ctx);

        if ctx.input(|input| input.viewport().close_requested())
''',
    '''        self.process_worker_messages();
        self.process_thumbnail_messages(ctx);
        self.observe_text_search_input();
        self.dispatch_text_search_if_due();
        self.process_text_search_results();

        if self.text_search_pending || self.text_search_due.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        if ctx.input(|input| input.viewport().close_requested())
''',
    "drive debounced text search",
)
path.write_text(text, encoding="utf-8")

print("FTS5 debounced text search patch applied")
