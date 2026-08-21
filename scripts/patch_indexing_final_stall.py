from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected exactly one match, found {count}: {old[:120]!r}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


embedding = Path("src/embedding.rs")
replace_once(
    embedding,
    "use std::sync::mpsc::{self, Sender};\n",
    "use std::sync::mpsc::{self, RecvTimeoutError, Receiver, Sender};\nuse std::time::{Duration, Instant};\n",
)
replace_once(
    embedding,
    "use std::path::PathBuf;\nuse std::sync::mpsc::{self, RecvTimeoutError, Receiver, Sender};\nuse std::time::{Duration, Instant};\n",
    "use std::path::PathBuf;\nuse std::sync::mpsc::{self, RecvTimeoutError, Receiver, Sender};\nuse std::time::{Duration, Instant};\n\nconst EMBEDDING_WAIT_POLL: Duration = Duration::from_secs(15);\nconst EMBEDDING_MAX_WAIT: Duration = Duration::from_secs(600);\n",
)
replace_once(
    embedding,
    "        response_rx\n            .recv()\n            .context(\"persistent CLIP service stopped unexpectedly\")?\n            .map_err(anyhow::Error::msg)\n",
    "        receive_with_deadline(&response_rx, EMBEDDING_MAX_WAIT)?\n            .map_err(anyhow::Error::msg)\n",
)
insert_before = "fn model_needs_reload(\n"
helper = r'''fn receive_with_deadline<T>(receiver: &Receiver<T>, max_wait: Duration) -> Result<T> {
    let started = Instant::now();
    loop {
        let remaining = max_wait.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            anyhow::bail!(
                "persistent CLIP service exceeded the {} second safety timeout; already committed index data is preserved, restart the application before retrying CLIP indexing",
                max_wait.as_secs()
            );
        }
        match receiver.recv_timeout(remaining.min(EMBEDDING_WAIT_POLL)) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("persistent CLIP service stopped unexpectedly")
            }
        }
    }
}

'''
replace_once(embedding, insert_before, helper + insert_before)

# Add a fast regression test for the watchdog helper without loading a model.
needle = "    #[test]\n    fn model_is_reused_until_threads_or_provider_change() {\n"
test = r'''    #[test]
    fn embedding_response_wait_has_a_bounded_timeout() {
        let (_tx, rx) = mpsc::channel::<()>();
        let started = Instant::now();
        let err = receive_with_deadline(&rx, Duration::from_millis(20)).unwrap_err();
        assert!(err.to_string().contains("safety timeout"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

'''
replace_once(embedding, needle, test + needle)

indexer = Path("src/indexer.rs")
replace_once(
    indexer,
    "    let total = paths.len();\n    let batch_size = indexing_settings.batch_size;\n    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {\n",
    "    let total = paths.len();\n    let batch_size = indexing_settings.batch_size;\n    let batch_total = total.div_ceil(batch_size);\n    for (batch_index, batch) in paths.chunks(batch_size).enumerate() {\n",
)
replace_once(
    indexer,
    "        let response = embedding_service\n            .embed_with_provider(\n",
    "        let batch_number = batch_index + 1;\n        let _ = tx.send(WorkerMessage::Status(format!(\n            \"CLIP batch {batch_number}/{batch_total}: embedding {} image{} on {}…\",\n            batch.len(),\n            if batch.len() == 1 { \"\" } else { \"s\" },\n            indexing_settings.clip_execution_provider.label()\n        )));\n        let response = embedding_service\n            .embed_with_provider(\n",
)
replace_once(
    indexer,
    "        {\n            let transaction = conn.transaction()?;\n            for (path, embedding) in batch.iter().zip(response.embeddings.iter()) {\n",
    "        let _ = tx.send(WorkerMessage::Status(format!(\n            \"CLIP batch {batch_number}/{batch_total}: inference complete; committing {} embedding{}…\",\n            response.embeddings.len(),\n            if response.embeddings.len() == 1 { \"\" } else { \"s\" }\n        )));\n        {\n            let transaction = conn.transaction()?;\n            for (path, embedding) in batch.iter().zip(response.embeddings.iter()) {\n",
)
replace_once(
    indexer,
    "        portable::sync_paths_from_session(conn, batch)?;\n        let done = ((batch_index + 1) * batch_size).min(total);\n",
    "        portable::sync_paths_from_session(conn, batch)?;\n        let done = ((batch_index + 1) * batch_size).min(total);\n        let _ = tx.send(WorkerMessage::Status(format!(\n            \"CLIP batch {batch_number}/{batch_total}: committed and synced ({done}/{total})\"\n        )));\n",
)
old_final = '''    for root in roots {
        control.wait_if_paused();
        if root.exists() {
            portable::replace_root_from_session(db_path, root)?;
        }
    }

    let _ = tx.send(WorkerMessage::Status(format!(
'''
new_final = '''    if removed > 0 {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Finalizing portable indexes after removing {removed} stale image{}…",
            if removed == 1 { "" } else { "s" }
        )));
        for root in roots {
            control.wait_if_paused();
            if root.exists() {
                let _ = tx.send(WorkerMessage::Status(format!(
                    "Portable cleanup sync: {}",
                    root.display()
                )));
                portable::replace_root_from_session(db_path, root)?;
            }
        }
    } else {
        let _ = tx.send(WorkerMessage::Status(
            "Portable indexes already synchronized incrementally; skipping redundant full-root rewrite"
                .to_owned(),
        ));
    }

    let _ = tx.send(WorkerMessage::Status(format!(
'''
replace_once(indexer, old_final, new_final)

print("patched indexing finalization watchdog and status reporting")
