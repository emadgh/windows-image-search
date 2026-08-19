from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/indexer.rs",
    "use crate::settings::IndexingSettings;",
    "use crate::settings::{ClipExecutionProvider, IndexingSettings};",
)

replace_once(
    "src/indexer.rs",
    """        let response = embedding_service
            .embed(batch.to_vec(), batch_size, indexing_settings.clip_threads)
            .with_context(|| format!(\"embedding image batch {}\", batch_index + 1))?;""",
    """        let response = embedding_service
            .embed_with_provider(
                batch.to_vec(),
                batch_size,
                indexing_settings.clip_threads,
                indexing_settings.clip_execution_provider,
            )
            .with_context(|| format!(\"embedding image batch {}\", batch_index + 1))?;""",
)

replace_once(
    "src/indexer.rs",
    """        if batch_index == 0 {
            let _ = tx.send(WorkerMessage::Status(if response.model_reloaded {
                format!(
                    \"CLIP model initialized with {} CPU thread{}; subsequent batches/searches will reuse it\",
                    indexing_settings.clip_threads,
                    if indexing_settings.clip_threads == 1 { \"\" } else { \"s\" }
                )
            } else {
                \"Reusing the already-loaded CLIP model\".to_owned()
            }));
        }""",
    """        if batch_index == 0 {
            let mut status = if response.model_reloaded {
                format!(
                    \"CLIP model initialized on {} with {} CPU thread{}; subsequent batches/searches will reuse it\",
                    response.active_provider.label(),
                    indexing_settings.clip_threads,
                    if indexing_settings.clip_threads == 1 { \"\" } else { \"s\" }
                )
            } else {
                format!(
                    \"Reusing the already-loaded CLIP model on {}\",
                    response.active_provider.label()
                )
            };
            if let Some(reason) = &response.fallback_reason {
                status.push_str(&format!(\" — {reason}\"));
            }
            let _ = tx.send(WorkerMessage::Status(status));
        }""",
)

replace_once(
    "src/indexer.rs",
    """        match query_clip_embedding(
            embedding_service,
            query_path,
            indexing_settings.clip_threads,
        ) {
            Ok((embedding, model_reloaded)) => {
                let _ = tx.send(WorkerMessage::Status(if model_reloaded {
                    \"CLIP model initialized for this query; future searches will reuse it\"
                        .to_owned()
                } else {
                    \"Reusing loaded CLIP model for query\".to_owned()
                }));
                Some(embedding)
            }""",
    """        match query_clip_embedding(
            embedding_service,
            query_path,
            indexing_settings.clip_threads,
            indexing_settings.clip_execution_provider,
        ) {
            Ok((embedding, model_reloaded, active_provider, fallback_reason)) => {
                let mut status = if model_reloaded {
                    format!(
                        \"CLIP model initialized on {} for this query; future searches will reuse it\",
                        active_provider.label()
                    )
                } else {
                    format!(\"Reusing loaded CLIP model on {} for query\", active_provider.label())
                };
                if let Some(reason) = fallback_reason {
                    status.push_str(&format!(\" — {reason}\"));
                }
                let _ = tx.send(WorkerMessage::Status(status));
                Some(embedding)
            }""",
)

replace_once(
    "src/indexer.rs",
    """fn query_clip_embedding(
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
        .context(\"CLIP returned no query embedding\")?;
    Ok((embedding, model_reloaded))
}""",
    """fn query_clip_embedding(
    embedding_service: &EmbeddingService,
    query_path: &Path,
    clip_threads: usize,
    requested_provider: ClipExecutionProvider,
) -> Result<(Vec<f32>, bool, ClipExecutionProvider, Option<String>)> {
    let response = embedding_service.embed_with_provider(
        vec![query_path.to_path_buf()],
        1,
        clip_threads,
        requested_provider,
    )?;
    let model_reloaded = response.model_reloaded;
    let active_provider = response.active_provider;
    let fallback_reason = response.fallback_reason.clone();
    let embedding = response
        .embeddings
        .into_iter()
        .next()
        .context(\"CLIP returned no query embedding\")?;
    Ok((
        embedding,
        model_reloaded,
        active_provider,
        fallback_reason,
    ))
}""",
)

replace_once(
    "src/ui/mod.rs",
    "use crate::settings::{self, IndexingSettings};",
    "use crate::settings::{self, ClipExecutionProvider, IndexingSettings};",
)

replace_once(
    "src/ui/mod.rs",
    """                ui.small(format!(
                    \"Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16.\",
                    if logical_threads == 1 { \"\" } else { \"s\" }
                ));""",
    """                ui.small(format!(
                    \"Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16 / Device CPU.\",
                    if logical_threads == 1 { \"\" } else { \"s\" }
                ));""",
)

replace_once(
    "src/ui/mod.rs",
    """                    let batch_changed = ui
                        .add(
                            egui::Slider::new(
                                &mut self.indexing_settings.batch_size,
                                1..=settings::MAX_BATCH_SIZE,
                            )
                            .text(\"Index / embedding batch size\"),
                        )
                        .changed();

                    if decode_changed || clip_changed || batch_changed {""",
    """                    let provider_before = self.indexing_settings.clip_execution_provider;
                    egui::ComboBox::from_label(\"CLIP execution provider\")
                        .selected_text(self.indexing_settings.clip_execution_provider.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.indexing_settings.clip_execution_provider,
                                ClipExecutionProvider::Cpu,
                                \"CPU (safe default)\",
                            );
                            ui.selectable_value(
                                &mut self.indexing_settings.clip_execution_provider,
                                ClipExecutionProvider::DirectMl,
                                \"DirectML (Windows GPU)\",
                            );
                        });
                    let provider_changed =
                        provider_before != self.indexing_settings.clip_execution_provider;
                    ui.small(
                        \"DirectML uses the same CLIP model and falls back to CPU automatically if the GPU provider cannot initialize.\",
                    );

                    let batch_changed = ui
                        .add(
                            egui::Slider::new(
                                &mut self.indexing_settings.batch_size,
                                1..=settings::MAX_BATCH_SIZE,
                            )
                            .text(\"Index / embedding batch size\"),
                        )
                        .changed();

                    if decode_changed || clip_changed || provider_changed || batch_changed {""",
)

replace_once(
    "src/ui/mod.rs",
    """                    self.status = format!(
                        \"Performance settings saved: decode {}, CLIP {}, batch {}\",
                        self.indexing_settings.decode_workers,
                        self.indexing_settings.clip_threads,
                        self.indexing_settings.batch_size
                    );""",
    """                    self.status = format!(
                        \"Performance settings saved: decode {}, CLIP {} threads on {}, batch {}\",
                        self.indexing_settings.decode_workers,
                        self.indexing_settings.clip_threads,
                        self.indexing_settings.clip_execution_provider.label(),
                        self.indexing_settings.batch_size
                    );""",
)
