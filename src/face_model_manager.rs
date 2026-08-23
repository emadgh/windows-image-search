use anyhow::{anyhow, bail, Context, Result};
use ort::session::Session;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceModelKind {
    YuNet,
    SFace,
}

impl FaceModelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::YuNet => "YuNet detector",
            Self::SFace => "SFace identity",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FaceModelManifest {
    pub kind: FaceModelKind,
    pub file_name: &'static str,
    pub source_url: &'static str,
    pub expected_size: u64,
    pub sha256_hex: &'static str,
    pub license: &'static str,
    pub source_label: &'static str,
}

pub const YUNET: FaceModelManifest = FaceModelManifest {
    kind: FaceModelKind::YuNet,
    file_name: "face_detection_yunet_2026may.onnx",
    source_url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2026may.onnx",
    expected_size: 229_738,
    sha256_hex: "ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0",
    license: "MIT",
    source_label: "OpenCV Zoo / YuNet 2026may",
};

pub const SFACE: FaceModelManifest = FaceModelManifest {
    kind: FaceModelKind::SFace,
    file_name: "face_recognition_sface_2021dec.onnx",
    source_url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx",
    expected_size: 38_696_353,
    sha256_hex: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    license: "Apache-2.0",
    source_label: "OpenCV Zoo / SFace 2021dec",
};

pub const MANIFESTS: [FaceModelManifest; 2] = [YUNET, SFACE];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedModelState {
    Missing,
    Ready,
    Invalid(String),
}

pub fn cache_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models").join("faces")
}

pub fn model_path(cache_dir: &Path, manifest: FaceModelManifest) -> PathBuf {
    cache_dir.join(manifest.file_name)
}

pub fn is_managed_path(path: &Path, cache_dir: &Path, manifest: FaceModelManifest) -> bool {
    path == model_path(cache_dir, manifest)
}

pub fn inspect(cache_dir: &Path, manifest: FaceModelManifest) -> ManagedModelState {
    let path = model_path(cache_dir, manifest);
    if !path.is_file() {
        return ManagedModelState::Missing;
    }
    match verify_model_file(&path, manifest, false) {
        Ok(()) => ManagedModelState::Ready,
        Err(err) => ManagedModelState::Invalid(format!("{err:#}")),
    }
}

pub fn verify_model_file(
    path: &Path,
    manifest: FaceModelManifest,
    validate_onnx: bool,
) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("reading model metadata {}", path.display()))?;
    if metadata.len() != manifest.expected_size {
        if looks_like_lfs_pointer(path)? {
            bail!(
                "{} is a Git-LFS pointer, not the ONNX model bytes",
                path.display()
            );
        }
        bail!(
            "{} has {} bytes; expected {}",
            path.display(),
            metadata.len(),
            manifest.expected_size
        );
    }

    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(manifest.sha256_hex) {
        bail!(
            "{} checksum mismatch: expected {}, got {}",
            manifest.file_name,
            manifest.sha256_hex,
            actual
        );
    }
    if validate_onnx {
        validate_onnx_session(path)?;
    }
    Ok(())
}

pub fn download_model(
    cache_dir: &Path,
    manifest: FaceModelManifest,
    force: bool,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("creating model cache {}", cache_dir.display()))?;
    let final_path = model_path(cache_dir, manifest);
    if !force && verify_model_file(&final_path, manifest, true).is_ok() {
        on_progress(manifest.expected_size, manifest.expected_size);
        return Ok(final_path);
    }

    let temp_path = final_path.with_extension("onnx.part");
    let _ = fs::remove_file(&temp_path);
    if force && final_path.is_file() {
        let _ = fs::remove_file(&final_path);
    }

    if cancel.load(Ordering::Relaxed) {
        bail!("model download cancelled");
    }

    let response = ureq::get(manifest.source_url)
        .set("User-Agent", "windows-image-search/0.3 face-model-manager")
        .call()
        .map_err(|err| anyhow!("downloading {}: {err}", manifest.file_name))?;
    let mut reader = response.into_reader();
    let mut output = File::create(&temp_path)
        .with_context(|| format!("creating temporary model {}", temp_path.display()))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut buffer = vec![0u8; 128 * 1024];

    let result = (|| -> Result<()> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("model download cancelled");
            }
            let read = reader
                .read(&mut buffer)
                .with_context(|| format!("reading download for {}", manifest.file_name))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .with_context(|| format!("writing temporary model {}", temp_path.display()))?;
            hasher.update(&buffer[..read]);
            downloaded = downloaded.saturating_add(read as u64);
            if downloaded > manifest.expected_size.saturating_add(1024) {
                bail!("downloaded response is larger than the expected model size");
            }
            on_progress(downloaded, manifest.expected_size);
        }
        output.flush()?;
        output.sync_all()?;

        if downloaded != manifest.expected_size {
            if looks_like_lfs_pointer(&temp_path)? {
                bail!("download returned a Git-LFS pointer instead of ONNX model bytes");
            }
            bail!(
                "downloaded {} bytes for {}; expected {}",
                downloaded,
                manifest.file_name,
                manifest.expected_size
            );
        }
        let digest = format!("{:x}", hasher.finalize());
        if !digest.eq_ignore_ascii_case(manifest.sha256_hex) {
            bail!(
                "downloaded {} failed SHA-256 verification: expected {}, got {}",
                manifest.file_name,
                manifest.sha256_hex,
                digest
            );
        }
        validate_onnx_session(&temp_path)?;
        Ok(())
    })();

    if let Err(err) = result {
        drop(output);
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    drop(output);

    if final_path.is_file() {
        fs::remove_file(&final_path)
            .with_context(|| format!("removing invalid cached model {}", final_path.display()))?;
    }
    fs::rename(&temp_path, &final_path).with_context(|| {
        format!(
            "installing verified model {} -> {}",
            temp_path.display(),
            final_path.display()
        )
    })?;
    Ok(final_path)
}

fn validate_onnx_session(path: &Path) -> Result<()> {
    Session::builder()
        .context("creating ONNX validation session builder")?
        .commit_from_file(path)
        .with_context(|| format!("validating ONNX model {}", path.display()))?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn looks_like_lfs_pointer(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut prefix = vec![0u8; 256];
    let read = file.read(&mut prefix)?;
    prefix.truncate(read);
    let text = String::from_utf8_lossy(&prefix);
    Ok(text.starts_with("version https://git-lfs.github.com/spec/v1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("wis-model-test-{name}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn managed_paths_are_stable() {
        let root = PathBuf::from("cache");
        assert_eq!(model_path(&root, YUNET), root.join(YUNET.file_name));
        assert!(is_managed_path(&root.join(YUNET.file_name), &root, YUNET));
    }

    #[test]
    fn lfs_pointer_is_rejected_before_normal_size_error() {
        let dir = test_dir("lfs");
        let path = dir.join("model.onnx");
        fs::write(
            &path,
            b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 42\n",
        )
        .unwrap();
        let manifest = FaceModelManifest {
            expected_size: 999,
            ..YUNET
        };
        let error = verify_model_file(&path, manifest, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Git-LFS pointer"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn wrong_size_and_checksum_are_not_ready() {
        let dir = test_dir("bad");
        let path = model_path(&dir, YUNET);
        fs::write(&path, b"not an onnx model").unwrap();
        assert!(matches!(
            inspect(&dir, YUNET),
            ManagedModelState::Invalid(_)
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
