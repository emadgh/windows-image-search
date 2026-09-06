use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub const MARKER: &str = "windows-image-search isolated benchmark workspace v1";

pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Option<PathBuf>> {
    let mut args = args.into_iter();
    let mut workspace = None;
    while let Some(arg) = args.next() {
        if arg == "--benchmark-workspace" {
            if workspace.is_some() {
                bail!("benchmark workspace specified more than once");
            }
            workspace = Some(PathBuf::from(
                args.next()
                    .context("--benchmark-workspace requires a directory")?,
            ));
        }
    }
    workspace.map(|path| validate(&path)).transpose()
}

fn validate(path: &Path) -> Result<PathBuf> {
    let root = path
        .canonicalize()
        .context("opening isolated benchmark workspace")?;
    if std::fs::read_to_string(root.join(".wis-benchmark-copy"))?.trim() != MARKER {
        bail!("workspace marker is missing or invalid; prepare a database copy first");
    }
    let database = root.join("index.sqlite3").canonicalize()?;
    if !database.starts_with(&root) {
        bail!("benchmark database resolves outside workspace");
    }
    let models = root.join("models");
    if models.exists() && !models.canonicalize()?.starts_with(&root) {
        bail!("benchmark model cache resolves outside workspace");
    }
    Ok(root)
}

/// Build a benchmark vector bank from existing portable previews. Only the
/// explicitly marked workspace is writable; missing previews are not generated.
pub fn build_preview_vectors(workspace: &Path) -> Result<String> {
    use crate::{db, embedding::EmbeddingService, thumbnail_cache};
    use std::time::Instant;
    let workspace = validate(workspace)?;
    let database = workspace.join("index.sqlite3");
    let images = db::load_image_summaries(&database)?;
    let mut conn = db::open(&database)?;
    let service = EmbeddingService::new(workspace.join("models"));
    let started = Instant::now();
    let mut completed = 0;
    let mut missing = 0;
    // Do not mix stored vectors of unknown input provenance with this bank.
    conn.execute("UPDATE images SET embedding = NULL", [])?;
    for chunk in images.chunks(32) {
        let selected: Vec<_> = chunk
            .iter()
            .filter_map(|record| {
                thumbnail_cache::valid_cache_path_for_root(&record.root, &record.path)
                    .map(|preview| (&record.path, preview))
            })
            .collect();
        missing += chunk.len() - selected.len();
        if selected.is_empty() {
            continue;
        }
        let response = service.embed(
            selected
                .iter()
                .map(|(_, preview)| preview.clone())
                .collect(),
            32,
            4,
        )?;
        if response.embeddings.len() != selected.len() {
            bail!("preview vector count mismatch");
        }
        let transaction = conn.transaction()?;
        for ((source, _), vector) in selected.iter().zip(&response.embeddings) {
            db::set_embedding(&transaction, source, vector)?;
        }
        transaction.commit()?;
        completed += selected.len();
        eprintln!(
            "preview vector bank: {completed}/{} indexed images; {missing} missing previews",
            images.len()
        );
    }
    Ok(format!("Windows Image Search Isolated Preview Vector Bank\nindexed_images={}\npreview_vectors={completed}\nmissing_previews={missing}\nprovider=cpu\nthreads=4\nbatch_size=32\ninput=existing_512px_portable_previews\nelapsed_ms={:.3}\nproduction_database_modified=false\n", images.len(), started.elapsed().as_secs_f64()*1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_is_explicit_and_requires_a_marked_existing_copy() {
        assert!(from_args(Vec::<String>::new()).unwrap().is_none());
        assert!(from_args(["--benchmark-workspace".into()]).is_err());
        let root = std::env::temp_dir().join(format!("wis-scope-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.sqlite3"), b"fixture").unwrap();
        assert!(validate(&root).is_err());
        assert!(build_preview_vectors(&root).is_err());
        assert_eq!(
            std::fs::read(root.join("index.sqlite3")).unwrap(),
            b"fixture"
        );
        std::fs::write(root.join(".wis-benchmark-copy"), MARKER).unwrap();
        assert_eq!(validate(&root).unwrap(), root.canonicalize().unwrap());
        assert!(from_args([
            "--benchmark-workspace".into(),
            root.display().to_string(),
            "--benchmark-workspace".into(),
            root.display().to_string()
        ])
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
