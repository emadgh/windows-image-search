use crate::settings::ClipExecutionProvider;
use anyhow::{Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use ort::ep::DirectML;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

#[derive(Debug)]
pub struct EmbeddingResponse {
    pub embeddings: Vec<Vec<f32>>,
    pub model_reloaded: bool,
    pub active_provider: ClipExecutionProvider,
    pub fallback_reason: Option<String>,
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
        requested_provider: ClipExecutionProvider,
        response: Sender<std::result::Result<EmbeddingResponse, String>>,
    },
}

struct ModelState {
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
    active_provider: ClipExecutionProvider,
    fallback_reason: Option<String>,
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
                            requested_provider,
                            response,
                        } => {
                            let result = embed_paths(
                                &model_cache,
                                &mut state,
                                paths,
                                batch_size,
                                clip_threads,
                                requested_provider,
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

    /// Backward-compatible CPU path used by diagnostic code that deliberately
    /// benchmarks the historical production backend.
    pub fn embed(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
    ) -> Result<EmbeddingResponse> {
        self.embed_with_provider(paths, batch_size, clip_threads, ClipExecutionProvider::Cpu)
    }

    pub fn embed_with_provider(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        requested_provider: ClipExecutionProvider,
    ) -> Result<EmbeddingResponse> {
        if paths.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: Vec::new(),
                model_reloaded: false,
                active_provider: requested_provider,
                fallback_reason: None,
            });
        }

        let (response_tx, response_rx) = mpsc::channel();
        self.tx
            .send(Command::Embed {
                paths,
                batch_size: batch_size.max(1),
                clip_threads: clip_threads.max(1),
                requested_provider,
                response: response_tx,
            })
            .context("sending work to persistent CLIP service")?;

        response_rx
            .recv()
            .context("persistent CLIP service stopped unexpectedly")?
            .map_err(anyhow::Error::msg)
    }
}

fn model_needs_reload(
    current: Option<(usize, ClipExecutionProvider)>,
    requested_threads: usize,
    requested_provider: ClipExecutionProvider,
) -> bool {
    current != Some((requested_threads.max(1), requested_provider))
}

fn image_options(model_cache: &std::path::Path, clip_threads: usize) -> ImageInitOptions {
    ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
        .with_cache_dir(model_cache.to_path_buf())
        .with_show_download_progress(true)
        .with_intra_threads(clip_threads.max(1))
}

fn load_model(
    model_cache: &std::path::Path,
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
) -> Result<(ImageEmbedding, ClipExecutionProvider, Option<String>)> {
    std::fs::create_dir_all(model_cache)?;

    match requested_provider {
        ClipExecutionProvider::Cpu => {
            let model = ImageEmbedding::try_new(image_options(model_cache, clip_threads))
                .context("loading CLIP image model on CPU")?;
            Ok((model, ClipExecutionProvider::Cpu, None))
        }
        ClipExecutionProvider::DirectMl => {
            let directml_options = image_options(model_cache, clip_threads)
                .with_execution_providers(vec![DirectML::default().into()]);
            match ImageEmbedding::try_new(directml_options) {
                Ok(model) => Ok((model, ClipExecutionProvider::DirectMl, None)),
                Err(directml_error) => {
                    let fallback_reason =
                        format!("DirectML unavailable; using CPU fallback: {directml_error:#}");
                    let cpu_model = ImageEmbedding::try_new(image_options(model_cache, clip_threads))
                        .with_context(|| {
                            format!(
                                "DirectML initialization failed ({directml_error:#}) and CPU fallback could not initialize"
                            )
                        })?;
                    Ok((cpu_model, ClipExecutionProvider::Cpu, Some(fallback_reason)))
                }
            }
        }
    }
}

fn embed_paths(
    model_cache: &std::path::Path,
    state: &mut Option<ModelState>,
    paths: Vec<PathBuf>,
    batch_size: usize,
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
) -> Result<EmbeddingResponse> {
    let clip_threads = clip_threads.max(1);
    let reload = model_needs_reload(
        state
            .as_ref()
            .map(|state| (state.clip_threads, state.requested_provider)),
        clip_threads,
        requested_provider,
    );

    if reload {
        let (model, active_provider, fallback_reason) =
            load_model(model_cache, clip_threads, requested_provider)?;
        *state = Some(ModelState {
            clip_threads,
            requested_provider,
            active_provider,
            fallback_reason,
            model,
        });
    }

    let state = state
        .as_mut()
        .context("persistent CLIP model was not initialized")?;
    let mut embeddings = state
        .model
        .embed(paths, Some(batch_size.max(1)))
        .context("embedding images with persistent CLIP model")?;
    for embedding in &mut embeddings {
        normalize_embedding(embedding);
    }

    Ok(EmbeddingResponse {
        embeddings,
        model_reloaded: reload,
        active_provider: state.active_provider,
        fallback_reason: state.fallback_reason.clone(),
    })
}

fn normalize_embedding(values: &mut [f32]) {
    let norm_sq = values.iter().map(|value| value * value).sum::<f32>();
    if norm_sq <= f32::EPSILON {
        return;
    }
    let inverse = norm_sq.sqrt().recip();
    for value in values {
        *value *= inverse;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_normalization_produces_unit_vectors() {
        let mut values = vec![3.0, 4.0];
        normalize_embedding(&mut values);
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn model_is_reused_until_threads_or_provider_change() {
        assert!(model_needs_reload(None, 4, ClipExecutionProvider::Cpu));
        assert!(!model_needs_reload(
            Some((4, ClipExecutionProvider::Cpu)),
            4,
            ClipExecutionProvider::Cpu
        ));
        assert!(model_needs_reload(
            Some((4, ClipExecutionProvider::Cpu)),
            2,
            ClipExecutionProvider::Cpu
        ));
        assert!(model_needs_reload(
            Some((4, ClipExecutionProvider::Cpu)),
            4,
            ClipExecutionProvider::DirectMl
        ));
        assert!(!model_needs_reload(
            Some((1, ClipExecutionProvider::DirectMl)),
            0,
            ClipExecutionProvider::DirectMl
        ));
    }
}
