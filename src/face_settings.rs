use crate::face_sface_adapter::SFaceExecutionProvider;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceEmbeddingSettings {
    pub model_path: PathBuf,
    pub provider: SFaceExecutionProvider,
}

impl Default for FaceEmbeddingSettings {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            provider: SFaceExecutionProvider::Cpu,
        }
    }
}

impl FaceEmbeddingSettings {
    pub fn configured(&self) -> bool {
        !self.model_path.as_os_str().is_empty()
    }

    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            SFaceExecutionProvider::Cpu => "CPU",
            SFaceExecutionProvider::DirectMl => "DirectML (GPU)",
        }
    }
}

pub fn load(path: &Path) -> FaceEmbeddingSettings {
    let mut settings = FaceEmbeddingSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "model_path" => settings.model_path = PathBuf::from(value.trim()),
            "provider" => {
                settings.provider = match value.trim().to_ascii_lowercase().as_str() {
                    "directml" | "dml" | "gpu" => SFaceExecutionProvider::DirectMl,
                    _ => SFaceExecutionProvider::Cpu,
                }
            }
            _ => {}
        }
    }
    settings
}

pub fn save(path: &Path, settings: &FaceEmbeddingSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating face settings directory {}", parent.display()))?;
    }
    let content = format!(
        "model_path={}\nprovider={}\n",
        settings.model_path.display(),
        settings.provider.as_str()
    );
    std::fs::write(path, content)
        .with_context(|| format!("writing face settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-face-settings-{label}-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn face_settings_round_trip_external_path_and_provider() {
        let path = temp_path("roundtrip");
        let expected = FaceEmbeddingSettings {
            model_path: PathBuf::from(r"D:\models\face\sface.onnx"),
            provider: SFaceExecutionProvider::DirectMl,
        };
        save(&path, &expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_provider_falls_back_to_cpu() {
        let path = temp_path("provider");
        std::fs::write(&path, "model_path=C:\\model.onnx\nprovider=unknown\n").unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.provider, SFaceExecutionProvider::Cpu);
        let _ = std::fs::remove_file(path);
    }
}
