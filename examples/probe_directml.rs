//! Compare automatic DirectML discovery with explicit DXGI device selection.
//! Only a marked benchmark workspace is accepted; no application database opens.
use anyhow::{bail, Context, Result};
use fastembed::{ImageEmbedding, ImageEmbeddingModel, ImageInitOptions};
use ort::ep::DirectML;
use std::path::PathBuf;

fn main() -> Result<()> {
    let workspace = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("pass an isolated benchmark workspace")?,
    )
    .canonicalize()?;
    if std::fs::read_to_string(workspace.join(".wis-benchmark-copy"))?.trim()
        != "windows-image-search isolated benchmark workspace v1"
    {
        bail!("isolated workspace marker required");
    }
    let cache = workspace.join("models").canonicalize()?;
    if !cache.starts_with(&workspace) {
        bail!("model cache resolves outside workspace");
    }
    let mut inputs: Vec<_> = std::fs::read_dir(workspace.join("inputs"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()?;
    inputs.sort();
    let input = inputs
        .into_iter()
        .find(|path| path.extension().is_some_and(|ext| ext == "jpg"))
        .context("workspace inputs must contain a JPEG preview")?;
    for (label, provider) in [
        ("automatic", DirectML::default()),
        ("dxgi_0", DirectML::default().with_device_id(0)),
        ("dxgi_1", DirectML::default().with_device_id(1)),
    ] {
        let options = ImageInitOptions::new(ImageEmbeddingModel::ClipVitB32)
            .with_cache_dir(cache.clone())
            .with_intra_threads(4)
            .with_execution_providers(vec![provider.build().error_on_failure()]);
        let result = ImageEmbedding::try_new(options)
            .and_then(|mut model| model.embed(vec![input.clone()], Some(1)));
        match result {
            Ok(vectors) => println!(
                "{label}.status=ok\n{label}.vectors={}\n{label}.dimension={}",
                vectors.len(),
                vectors.first().map_or(0, Vec::len)
            ),
            Err(error) => println!(
                "{label}.status=error\n{label}.error={}",
                format!("{error:#}").replace(['\n', '\r'], " ")
            ),
        }
    }
    Ok(())
}
