use super::yunet_adapter::{
    YuNetExecutionProvider, DEFAULT_NMS_THRESHOLD, DEFAULT_SCORE_THRESHOLD, DEFAULT_TOP_K,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct FaceDetectorSettings {
    pub model_path: PathBuf,
    pub provider: YuNetExecutionProvider,
    pub score_threshold: f32,
    pub nms_threshold: f32,
    pub top_k: usize,
}

impl Default for FaceDetectorSettings {
    fn default() -> Self {
        Self {
            model_path: PathBuf::new(),
            provider: YuNetExecutionProvider::Cpu,
            score_threshold: DEFAULT_SCORE_THRESHOLD,
            nms_threshold: DEFAULT_NMS_THRESHOLD,
            top_k: DEFAULT_TOP_K,
        }
    }
}

impl FaceDetectorSettings {
    pub fn configured(&self) -> bool {
        !self.model_path.as_os_str().is_empty()
    }

    pub fn provider_label(&self) -> &'static str {
        match self.provider {
            YuNetExecutionProvider::Cpu => "CPU",
            YuNetExecutionProvider::DirectMl => "DirectML (GPU)",
        }
    }

    pub fn sanitized(mut self) -> Self {
        self.score_threshold = if self.score_threshold.is_finite() {
            self.score_threshold.clamp(0.0, 1.0)
        } else {
            DEFAULT_SCORE_THRESHOLD
        };
        self.nms_threshold = if self.nms_threshold.is_finite() {
            self.nms_threshold.clamp(0.0, 1.0)
        } else {
            DEFAULT_NMS_THRESHOLD
        };
        self.top_k = self.top_k.clamp(1, 100_000);
        self
    }
}

pub fn load(path: &Path) -> FaceDetectorSettings {
    let mut settings = FaceDetectorSettings::default();
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
                    "directml" | "dml" | "gpu" => YuNetExecutionProvider::DirectMl,
                    _ => YuNetExecutionProvider::Cpu,
                }
            }
            "score_threshold" => {
                if let Ok(value) = value.trim().parse::<f32>() {
                    settings.score_threshold = value;
                }
            }
            "nms_threshold" => {
                if let Ok(value) = value.trim().parse::<f32>() {
                    settings.nms_threshold = value;
                }
            }
            "top_k" => {
                if let Ok(value) = value.trim().parse::<usize>() {
                    settings.top_k = value;
                }
            }
            _ => {}
        }
    }
    settings.sanitized()
}

pub fn save(path: &Path, settings: &FaceDetectorSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating YuNet settings directory {}", parent.display()))?;
    }
    let settings = settings.clone().sanitized();
    let content = format!(
        "model_path={}\nprovider={}\nscore_threshold={}\nnms_threshold={}\ntop_k={}\n",
        settings.model_path.display(),
        settings.provider.as_str(),
        settings.score_threshold,
        settings.nms_threshold,
        settings.top_k
    );
    std::fs::write(path, content)
        .with_context(|| format!("writing YuNet settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-yunet-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn detector_settings_round_trip() {
        let path = temp_path();
        let expected = FaceDetectorSettings {
            model_path: PathBuf::from(r"D:\models\yunet.onnx"),
            provider: YuNetExecutionProvider::DirectMl,
            score_threshold: 0.72,
            nms_threshold: 0.28,
            top_k: 2048,
        };
        save(&path, &expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unsafe_values_are_sanitized() {
        let settings = FaceDetectorSettings {
            score_threshold: f32::NAN,
            nms_threshold: 2.0,
            top_k: 0,
            ..FaceDetectorSettings::default()
        }
        .sanitized();
        assert_eq!(settings.score_threshold, DEFAULT_SCORE_THRESHOLD);
        assert_eq!(settings.nms_threshold, 1.0);
        assert_eq!(settings.top_k, 1);
    }
}
