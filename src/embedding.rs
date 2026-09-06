use crate::settings::ClipExecutionProvider;
use anyhow::{Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use ort::ep::DirectML;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::time::{Duration, Instant};

const EMBEDDING_WAIT_POLL: Duration = Duration::from_secs(15);
const EMBEDDING_MAX_WAIT: Duration = Duration::from_secs(600);
const EMBEDDING_QUEUE_CAPACITY: usize = 8;
const MAX_INTERACTIVE_BURST: usize = 4;

#[derive(Debug)]
pub struct EmbeddingResponse {
    pub embeddings: Vec<Vec<f32>>,
    pub model_reloaded: bool,
    pub active_provider: ClipExecutionProvider,
    pub fallback_reason: Option<String>,
}

#[derive(Clone)]
pub struct EmbeddingService {
    tx: SyncSender<Command>,
}

enum Command {
    Text {
        text: String,
        response: Sender<std::result::Result<Vec<f32>, String>>,
        request: crate::search_request::SearchRequest,
    },
    Embed {
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        requested_provider: ClipExecutionProvider,
        response: Sender<std::result::Result<EmbeddingResponse, String>>,
        request: Option<crate::search_request::SearchRequest>,
    },
}

impl Command {
    fn interactive(&self) -> bool {
        if matches!(self, Self::Text { .. }) {
            return true;
        }
        matches!(
            self,
            Self::Embed {
                request: Some(_),
                ..
            }
        )
    }

    fn cancelled(&self) -> bool {
        match self {
            Self::Text { request, .. } => request.check().is_err(),
            Self::Embed { request, .. } => request
                .as_ref()
                .is_some_and(|request| request.check().is_err()),
        }
    }
}

// Called only at inference boundaries. Keep the model/session on one worker;
// bounded bursts prioritize UI work without starving indexing indefinitely.
fn next_queued_command(
    queue: &mut VecDeque<Command>,
    interactive_burst: &mut usize,
) -> Option<Command> {
    queue.retain(|command| !command.cancelled());
    let interactive = queue.iter().position(Command::interactive);
    let background = queue.iter().position(|command| !command.interactive());
    let index = if *interactive_burst >= MAX_INTERACTIVE_BURST {
        background.or(interactive)
    } else {
        interactive.or(background)
    }?;
    let command = queue.remove(index)?;
    if command.interactive() {
        *interactive_burst = (*interactive_burst + 1).min(MAX_INTERACTIVE_BURST);
    } else {
        *interactive_burst = 0;
    }
    Some(command)
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
        let (tx, rx) = mpsc::sync_channel::<Command>(EMBEDDING_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("clip-embedding-service".to_owned())
            .spawn(move || {
                let mut state: Option<ModelState> = None;
                let mut text_model: Option<fastembed::TextEmbedding> = None;
                let mut queue = VecDeque::new();
                let mut interactive_burst = 0;
                loop {
                    if queue.is_empty() {
                        let Ok(command) = rx.recv() else {
                            break;
                        };
                        queue.push_back(command);
                    }
                    while queue.len() < EMBEDDING_QUEUE_CAPACITY {
                        let Ok(command) = rx.try_recv() else {
                            break;
                        };
                        queue.push_back(command);
                    }
                    let Some(command) = next_queued_command(&mut queue, &mut interactive_burst)
                    else {
                        continue;
                    };
                    match command {
                        Command::Text {
                            text,
                            response,
                            request,
                        } => {
                            let result = (|| -> Result<Vec<f32>> {
                                request.check()?;
                                if text_model.is_none() {
                                    text_model = Some(
                                        fastembed::TextEmbedding::try_new(
                                            fastembed::TextInitOptions::new(
                                                fastembed::EmbeddingModel::ClipVitB32,
                                            )
                                            .with_cache_dir(model_cache.clone())
                                            .with_max_length(77)
                                            .with_intra_threads(2),
                                        )
                                        .context(
                                            "loading the matching CLIP ViT-B/32 text encoder",
                                        )?,
                                    );
                                }
                                request.check()?;
                                let mut vector = text_model
                                    .as_mut()
                                    .unwrap()
                                    .embed(vec![text], Some(1))?
                                    .pop()
                                    .context("CLIP text encoder returned no embedding")?;
                                if vector.len() != 512
                                    || vector.iter().any(|value| !value.is_finite())
                                {
                                    anyhow::bail!(
                                        "CLIP text encoder returned an incompatible vector"
                                    );
                                }
                                normalize_embedding(&mut vector);
                                Ok(vector)
                            })()
                            .map_err(|error| format!("{error:#}"));
                            let _ = response.send(result);
                        }
                        Command::Embed {
                            paths,
                            batch_size,
                            clip_threads,
                            requested_provider,
                            response,
                            request,
                        } => {
                            if request
                                .as_ref()
                                .is_some_and(|request| request.check().is_err())
                            {
                                continue;
                            }
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

    pub fn embed_description(
        &self,
        text: String,
        request: crate::search_request::SearchRequest,
    ) -> Result<Vec<f32>> {
        if text.trim().is_empty() {
            anyhow::bail!("Describe the image to search for");
        }
        request.check()?;
        let (response, rx) = mpsc::channel();
        self.tx
            .try_send(Command::Text {
                text,
                response,
                request: request.clone(),
            })
            .context("CLIP query queue is full or unavailable")?;
        receive_cancellable(&rx, EMBEDDING_MAX_WAIT, Some(&request))?.map_err(anyhow::Error::msg)
    }

    pub fn embed_with_provider(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        requested_provider: ClipExecutionProvider,
    ) -> Result<EmbeddingResponse> {
        self.embed_request(paths, batch_size, clip_threads, requested_provider, None)
    }

    pub fn embed_with_provider_cancellable(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        requested_provider: ClipExecutionProvider,
        request: crate::search_request::SearchRequest,
    ) -> Result<EmbeddingResponse> {
        self.embed_request(
            paths,
            batch_size,
            clip_threads,
            requested_provider,
            Some(request),
        )
    }

    fn embed_request(
        &self,
        paths: Vec<PathBuf>,
        batch_size: usize,
        clip_threads: usize,
        requested_provider: ClipExecutionProvider,
        request: Option<crate::search_request::SearchRequest>,
    ) -> Result<EmbeddingResponse> {
        if let Some(request) = &request {
            request.check()?;
        }
        if paths.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: Vec::new(),
                model_reloaded: false,
                active_provider: requested_provider,
                fallback_reason: None,
            });
        }

        let (response_tx, response_rx) = mpsc::channel();
        let command = Command::Embed {
            paths,
            batch_size: batch_size.max(1),
            clip_threads: clip_threads.max(1),
            requested_provider,
            response: response_tx,
            request: request.clone(),
        };
        if request.is_some() {
            self.tx
                .try_send(command)
                .context("CLIP query queue is full or unavailable; retry the search")?;
        } else {
            self.tx
                .send(command)
                .context("sending work to persistent CLIP service")?;
        }

        receive_cancellable(&response_rx, EMBEDDING_MAX_WAIT, request.as_ref())?
            .map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
fn receive_with_deadline<T>(receiver: &Receiver<T>, max_wait: Duration) -> Result<T> {
    receive_cancellable(receiver, max_wait, None)
}

fn receive_cancellable<T>(
    receiver: &Receiver<T>,
    max_wait: Duration,
    request: Option<&crate::search_request::SearchRequest>,
) -> Result<T> {
    let started = Instant::now();
    loop {
        if let Some(request) = request {
            request.check()?;
        }
        let remaining = max_wait.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            anyhow::bail!(
                "persistent CLIP service exceeded the {} second safety timeout; already committed index data is preserved, restart the application before retrying CLIP indexing",
                max_wait.as_secs()
            );
        }
        let poll = if request.is_some() {
            Duration::from_millis(100)
        } else {
            EMBEDDING_WAIT_POLL
        };
        match receiver.recv_timeout(remaining.min(poll)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("persistent CLIP service stopped unexpectedly")
            }
        }
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
                .with_execution_providers(vec![DirectML::default().build().error_on_failure()]);
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

    fn queued_command(
        name: &str,
        request: Option<crate::search_request::SearchRequest>,
    ) -> Command {
        let (response, _) = mpsc::channel();
        Command::Embed {
            paths: vec![PathBuf::from(name)],
            batch_size: 1,
            clip_threads: 1,
            requested_provider: ClipExecutionProvider::Cpu,
            response,
            request,
        }
    }

    fn command_name(command: Command) -> PathBuf {
        match command {
            Command::Embed { paths, .. } => paths[0].clone(),
            Command::Text { text, .. } => PathBuf::from(text),
        }
    }

    #[test]
    fn interactive_query_overtakes_queued_index_batches() {
        let mut session = crate::search_request::SearchSession::default();
        let mut queue = VecDeque::from([
            queued_command("batch-1", None),
            queued_command("batch-2", None),
            queued_command("query", Some(session.start())),
        ]);
        let mut burst = 0;
        assert_eq!(
            command_name(next_queued_command(&mut queue, &mut burst).unwrap()),
            PathBuf::from("query")
        );
        assert_eq!(
            command_name(next_queued_command(&mut queue, &mut burst).unwrap()),
            PathBuf::from("batch-1")
        );
        assert_eq!(
            command_name(next_queued_command(&mut queue, &mut burst).unwrap()),
            PathBuf::from("batch-2")
        );
    }

    #[test]
    fn continuous_queries_still_allow_indexing_to_progress() {
        let mut sessions: Vec<_> = (0..MAX_INTERACTIVE_BURST + 1)
            .map(|_| crate::search_request::SearchSession::default())
            .collect();
        let mut queue = VecDeque::from([queued_command("index-batch", None)]);
        for session in &mut sessions {
            queue.push_back(queued_command("query", Some(session.start())));
        }
        let mut burst = 0;
        for _ in 0..MAX_INTERACTIVE_BURST {
            assert!(next_queued_command(&mut queue, &mut burst)
                .unwrap()
                .interactive());
        }
        assert_eq!(
            command_name(next_queued_command(&mut queue, &mut burst).unwrap()),
            PathBuf::from("index-batch")
        );
        assert!(next_queued_command(&mut queue, &mut burst)
            .unwrap()
            .interactive());
    }

    #[test]
    fn cancelled_queued_query_does_not_delay_indexing() {
        let mut session = crate::search_request::SearchSession::default();
        let mut queue = VecDeque::from([
            queued_command("cancelled-query", Some(session.start())),
            queued_command("index-batch", None),
        ]);
        session.cancel();
        let mut burst = 0;
        assert_eq!(
            command_name(next_queued_command(&mut queue, &mut burst).unwrap()),
            PathBuf::from("index-batch")
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn cancelled_embedding_wait_does_not_accept_a_queued_response() {
        let mut session = crate::search_request::SearchSession::default();
        let request = session.start();
        let (tx, rx) = mpsc::channel();
        tx.send(42).unwrap();
        session.cancel();
        let error = receive_cancellable(&rx, Duration::from_secs(1), Some(&request)).unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn cancellation_releases_an_embedding_wait_without_a_model_response() {
        let mut session = crate::search_request::SearchSession::default();
        let request = session.start();
        let (_tx, rx) = mpsc::channel::<()>();
        let waiter = std::thread::spawn(move || {
            receive_cancellable(&rx, Duration::from_secs(2), Some(&request))
        });
        session.cancel();
        let error = waiter.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn embedding_normalization_produces_unit_vectors() {
        let mut values = vec![3.0, 4.0];
        normalize_embedding(&mut values);
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn embedding_response_wait_has_a_bounded_timeout() {
        let (_tx, rx) = mpsc::channel::<()>();
        let started = Instant::now();
        let err = receive_with_deadline(&rx, Duration::from_millis(20)).unwrap_err();
        assert!(err.to_string().contains("safety timeout"));
        assert!(started.elapsed() < Duration::from_secs(1));
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
