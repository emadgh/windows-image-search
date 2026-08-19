from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# New persistent embedding service. The FastEmbed model lives only on this
# dedicated worker thread and is reused across indexing batches and searches.
# -----------------------------------------------------------------------------
Path("src/embedding.rs").write_text(
    r'''use anyhow::{Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

#[derive(Debug)]
pub struct EmbeddingResponse {
    pub embeddings: Vec<Vec<f32>>,
    pub model_reloaded: bool,
}

#[derive(Clone)]
pub struct EmbeddingService {
    tx: Sender<Command>,
}

enum Command {
    Embed {
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        response: Sender<std::result::Result<EmbeddingResponse, String>>,
    },
}

struct ModelState {
    clip_threads: usize,
    model: ImageEmbedding,
}

impl EmbeddingService {
    pub fn new(model_cache: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<Command>();
        std::thread::Builder::new()
            .name("clip-embedding-service".to_owned())
            .spawn(move || {
                let mut state: Option<ModelState> = None;
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Embed {
                            paths,
                            batch_size,
                            clip_threads,
                            response,
                        } => {
                            let result = embed_paths(
                                &model_cache,
                                &mut state,
                                paths,
                                batch_size,
                                clip_threads,
                            )
                            .map_err(|err| format!("{err:#}"));
                            let _ = response.send(result);
                        }
                    }
                }
            })
            .expect("creating persistent CLIP embedding worker");
        Self { tx }
    }

    pub fn embed(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
    ) -> Result<EmbeddingResponse> {
        if paths.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: Vec::new(),
                model_reloaded: false,
            });
        }

        let (response_tx, response_rx) = mpsc::channel();
        self.tx
            .send(Command::Embed {
                paths,
                batch_size: batch_size.max(1),
                clip_threads: clip_threads.max(1),
                response: response_tx,
            })
            .context("sending work to persistent CLIP service")?;

        response_rx
            .recv()
            .context("persistent CLIP service stopped unexpectedly")?
            .map_err(anyhow::Error::msg)
    }
}

fn model_needs_reload(current_threads: Option<usize>, requested_threads: usize) -> bool {
    current_threads != Some(requested_threads.max(1))
}

fn embed_paths(
    model_cache: &std::path::Path,
    state: &mut Option<ModelState>,
    paths: Vec<PathBuf>,
    batch_size: usize,
    clip_threads: usize,
) -> Result<EmbeddingResponse> {
    let clip_threads = clip_threads.max(1);
    let reload = model_needs_reload(state.as_ref().map(|state| state.clip_threads), clip_threads);

    if reload {
        std::fs::create_dir_all(model_cache)?;
        let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
            .with_cache_dir(model_cache.to_path_buf())
            .with_show_download_progress(true)
            .with_intra_threads(clip_threads);
        let model = ImageEmbedding::try_new(options).context("loading CLIP image model")?;
        *state = Some(ModelState {
            clip_threads,
            model,
        });
    }

    let model = &mut state
        .as_mut()
        .context("persistent CLIP model was not initialized")?
        .model;
    let embeddings = model
        .embed(paths, Some(batch_size.max(1)))
        .context("embedding images with persistent CLIP model")?;

    Ok(EmbeddingResponse {
        embeddings,
        model_reloaded: reload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_is_reused_until_thread_setting_changes() {
        assert!(model_needs_reload(None, 4));
        assert!(!model_needs_reload(Some(4), 4));
        assert!(model_needs_reload(Some(4), 2));
        assert!(!model_needs_reload(Some(1), 0));
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
    "mod db;\nmod indexer;\n",
    "mod db;\nmod embedding;\nmod indexer;\n",
    "embedding module",
)
path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# indexer.rs
# -----------------------------------------------------------------------------
path = Path("src/indexer.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::db::{self, ImageRecord};\nuse crate::metadata;\n",
    "use crate::db::{self, ImageRecord};\nuse crate::embedding::EmbeddingService;\nuse crate::metadata;\n",
    "indexer embedding import",
)
text = replace_once(
    text,
    "use anyhow::{Context, Result};\nuse fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};\n",
    "use anyhow::{bail, Context, Result};\n",
    "remove direct FastEmbed imports",
)

text = replace_once(
    text,
    '''pub fn spawn_rescan(
    db_path: PathBuf,
    model_cache: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(&db_path, &model_cache, &roots, indexing_settings, &tx);
''',
    '''pub fn spawn_rescan(
    db_path: PathBuf,
    roots: Vec<PathBuf>,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let result = rescan(
            &db_path,
            &roots,
            indexing_settings,
            &embedding_service,
            &tx,
        );
''',
    "spawn_rescan persistent service",
)

text = replace_once(
    text,
    '''fn rescan(
    db_path: &Path,
    model_cache: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    '''fn rescan(
    db_path: &Path,
    roots: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
    "rescan persistent service",
)

text = replace_once(
    text,
    '''        if let Err(err) = build_embeddings(&mut conn, model_cache, &missing, indexing_settings, tx)
        {
''',
    '''        if let Err(err) = build_embeddings(
            &mut conn,
            &missing,
            indexing_settings,
            embedding_service,
            tx,
        ) {
''',
    "rescan embedding service call",
)

old_build_start = text.index("fn build_embeddings(")
old_build_end = text.index("\npub fn spawn_similarity_search", old_build_start)
old_build = text[old_build_start:old_build_end]
new_build = '''fn build_embeddings(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let _ = tx.send(WorkerMessage::Status(
        "Using persistent CLIP embedding service…".to_owned(),
    ));

    let total = paths.len();
    let batch_size = indexing_settings.batch_size;
    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {
        let response = embedding_service
            .embed(
                batch.to_vec(),
                batch_size,
                indexing_settings.clip_threads,
            )
            .with_context(|| format!("embedding image batch {}", batch_index + 1))?;
        if response.embeddings.len() != batch.len() {
            bail!(
                "CLIP returned {} embeddings for {} input images",
                response.embeddings.len(),
                batch.len()
            );
        }
        if batch_index == 0 {
            let _ = tx.send(WorkerMessage::Status(if response.model_reloaded {
                format!(
                    "CLIP model initialized with {} CPU thread{}; subsequent batches/searches will reuse it",
                    indexing_settings.clip_threads,
                    if indexing_settings.clip_threads == 1 { "" } else { "s" }
                )
            } else {
                "Reusing the already-loaded CLIP model".to_owned()
            }));
        }

        {
            let transaction = conn.transaction()?;
            for (path, embedding) in batch.iter().zip(response.embeddings.iter()) {
                db::set_embedding(&transaction, path, embedding)?;
            }
            transaction.commit()?;
        }
        let done = ((batch_index + 1) * batch_size).min(total);
        let _ = tx.send(WorkerMessage::Status(format!(
            "Building CLIP index: {done}/{total} (committed; persistent model)"
        )));
    }
    Ok(())
}
'''
text = text[:old_build_start] + new_build + text[old_build_end:]

text = replace_once(
    text,
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    model_cache: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    tx: Sender<WorkerMessage>,
) {
''',
    '''pub fn spawn_similarity_search(
    db_path: PathBuf,
    query_path: PathBuf,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: EmbeddingService,
    tx: Sender<WorkerMessage>,
) {
''',
    "spawn similarity persistent service",
)
text = replace_once(
    text,
    '''        match similarity_search(
            &db_path,
            &model_cache,
            &query_path,
            settings,
            indexing_settings,
            &tx,
        ) {
''',
    '''        match similarity_search(
            &db_path,
            &query_path,
            settings,
            indexing_settings,
            &embedding_service,
            &tx,
        ) {
''',
    "similarity persistent service call",
)

text = replace_once(
    text,
    '''fn similarity_search(
    db_path: &Path,
    model_cache: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {
''',
    '''fn similarity_search(
    db_path: &Path,
    query_path: &Path,
    settings: SimilaritySettings,
    indexing_settings: IndexingSettings,
    embedding_service: &EmbeddingService,
    tx: &Sender<WorkerMessage>,
) -> Result<Vec<ImageRecord>> {
''',
    "similarity persistent service signature",
)

text = replace_once(
    text,
    '''    let query_embedding =
        match query_clip_embedding(model_cache, query_path, indexing_settings.clip_threads) {
            Ok(embedding) => Some(embedding),
            Err(err) => {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "CLIP unavailable; using texture/color similarity only ({err})"
                )));
                None
            }
        };
''',
    '''    let query_embedding = match query_clip_embedding(
        embedding_service,
        query_path,
        indexing_settings.clip_threads,
    ) {
        Ok((embedding, model_reloaded)) => {
            let _ = tx.send(WorkerMessage::Status(if model_reloaded {
                "CLIP model initialized for this query; future searches will reuse it".to_owned()
            } else {
                "Reusing loaded CLIP model for query".to_owned()
            }));
            Some(embedding)
        }
        Err(err) => {
            let _ = tx.send(WorkerMessage::Status(format!(
                "CLIP unavailable; using texture/color similarity only ({err})"
            )));
            None
        }
    };
''',
    "query persistent service",
)

query_start = text.index("fn query_clip_embedding(")
query_end = text.index("\nfn passes_color_gate", query_start)
text = text[:query_start] + '''fn query_clip_embedding(
    embedding_service: &EmbeddingService,
    query_path: &Path,
    clip_threads: usize,
) -> Result<(Vec<f32>, bool)> {
    let response = embedding_service.embed(vec![query_path.to_path_buf()], 1, clip_threads)?;
    let model_reloaded = response.model_reloaded;
    let embedding = response
        .embeddings
        .into_iter()
        .next()
        .context("CLIP returned no query embedding")?;
    Ok((embedding, model_reloaded))
}
''' + text[query_end:]

if "ImageEmbedding" in text or "ImageEmbeddingModel" in text or "ImageInitOptions" in text:
    raise SystemExit("direct FastEmbed model construction remains in indexer.rs")
if "model_cache" in text:
    raise SystemExit("model_cache still threaded through indexer.rs")

path.write_text(text, encoding="utf-8")


# -----------------------------------------------------------------------------
# ui/mod.rs
# -----------------------------------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use crate::db::{self, ImageRecord};\nuse crate::indexer::{self, WorkerMessage};\n",
    "use crate::db::{self, ImageRecord};\nuse crate::embedding::EmbeddingService;\nuse crate::indexer::{self, WorkerMessage};\n",
    "UI embedding import",
)
text = replace_once(
    text,
    '''    pub(super) db_path: PathBuf,
    pub(super) model_cache: PathBuf,
    pub(super) roots: Vec<PathBuf>,
''',
    '''    pub(super) db_path: PathBuf,
    embedding_service: EmbeddingService,
    pub(super) roots: Vec<PathBuf>,
''',
    "UI embedding service field",
)
text = replace_once(
    text,
    '''        let indexing_settings = settings::load(&settings_path);
        let images = db::load_images(&db_path).unwrap_or_default();
''',
    '''        let indexing_settings = settings::load(&settings_path);
        let embedding_service = EmbeddingService::new(model_cache);
        let images = db::load_images(&db_path).unwrap_or_default();
''',
    "UI create embedding service",
)
text = replace_once(
    text,
    '''            db_path,
            model_cache,
            similarity_results: None,
''',
    '''            db_path,
            embedding_service,
            similarity_results: None,
''',
    "UI store embedding service",
)
text = replace_once(
    text,
    '''        indexer::spawn_rescan(
            self.db_path.clone(),
            self.model_cache.clone(),
            self.roots.clone(),
            self.indexing_settings,
            self.tx.clone(),
        );
''',
    '''        indexer::spawn_rescan(
            self.db_path.clone(),
            self.roots.clone(),
            self.indexing_settings,
            self.embedding_service.clone(),
            self.tx.clone(),
        );
''',
    "UI rescan persistent service",
)
text = replace_once(
    text,
    '''        indexer::spawn_similarity_search(
            self.db_path.clone(),
            self.model_cache.clone(),
            path,
            self.similarity_settings,
            self.indexing_settings,
            self.tx.clone(),
        );
''',
    '''        indexer::spawn_similarity_search(
            self.db_path.clone(),
            path,
            self.similarity_settings,
            self.indexing_settings,
            self.embedding_service.clone(),
            self.tx.clone(),
        );
''',
    "UI query persistent service",
)
if "self.model_cache" in text or "pub(super) model_cache" in text:
    raise SystemExit("UI model_cache field/reference remains")
path.write_text(text, encoding="utf-8")

print("Persistent CLIP service patch applied")
