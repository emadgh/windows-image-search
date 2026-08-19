use anyhow::{Context, Result};
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
