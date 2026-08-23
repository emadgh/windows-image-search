use anyhow::{Context, Result};
use std::path::Path;

pub const MAX_BATCH_SIZE: usize = 256;
pub const DEFAULT_MAX_FILE_SIZE_MIB: usize = 256;
pub const MAX_FILE_SIZE_MIB: usize = 16_384;
const MAX_CONFIGURED_THREADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipExecutionProvider {
    Cpu,
    DirectMl,
}

impl ClipExecutionProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::DirectMl => "DirectML (GPU)",
        }
    }

    fn config_value(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::DirectMl => "directml",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" => Some(Self::Cpu),
            "directml" | "dml" | "gpu" => Some(Self::DirectMl),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexingSettings {
    pub decode_workers: usize,
    pub clip_threads: usize,
    pub batch_size: usize,
    pub clip_execution_provider: ClipExecutionProvider,
    pub max_file_size_mib: usize,
}

impl Default for IndexingSettings {
    fn default() -> Self {
        let logical = logical_parallelism();
        Self {
            decode_workers: logical.saturating_sub(1).max(1).min(2),
            clip_threads: logical.saturating_sub(1).max(1).min(4),
            batch_size: 16,
            clip_execution_provider: ClipExecutionProvider::Cpu,
            max_file_size_mib: DEFAULT_MAX_FILE_SIZE_MIB,
        }
    }
}

impl IndexingSettings {
    pub fn sanitized(self) -> Self {
        Self {
            decode_workers: self.decode_workers.clamp(1, max_decode_workers()),
            clip_threads: self.clip_threads.clamp(1, max_clip_threads()),
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
            clip_execution_provider: self.clip_execution_provider,
            max_file_size_mib: self.max_file_size_mib.clamp(1, MAX_FILE_SIZE_MIB),
        }
    }

    pub fn max_file_size_bytes(self) -> u64 {
        (self.max_file_size_mib as u64).saturating_mul(1024 * 1024)
    }

    pub fn allows_file_size(self, bytes: u64) -> bool {
        bytes <= self.max_file_size_bytes()
    }
}

pub fn logical_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .max(1)
}

pub fn max_decode_workers() -> usize {
    logical_parallelism().min(MAX_CONFIGURED_THREADS).max(1)
}

pub fn max_clip_threads() -> usize {
    logical_parallelism().min(MAX_CONFIGURED_THREADS).max(1)
}

pub fn load(path: &Path) -> IndexingSettings {
    let mut settings = IndexingSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "decode_workers" => {
                if let Ok(value) = value.trim().parse::<usize>() {
                    settings.decode_workers = value;
                }
            }
            "clip_threads" => {
                if let Ok(value) = value.trim().parse::<usize>() {
                    settings.clip_threads = value;
                }
            }
            "batch_size" => {
                if let Ok(value) = value.trim().parse::<usize>() {
                    settings.batch_size = value;
                }
            }
            "clip_execution_provider" => {
                if let Some(provider) = ClipExecutionProvider::parse(value) {
                    settings.clip_execution_provider = provider;
                }
            }
            "max_file_size_mib" => {
                if let Ok(value) = value.trim().parse::<usize>() {
                    settings.max_file_size_mib = value;
                }
            }
            _ => {}
        }
    }

    settings.sanitized()
}

pub fn save(path: &Path, settings: IndexingSettings) -> Result<()> {
    let settings = settings.sanitized();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating settings directory {}", parent.display()))?;
    }
    let content = format!(
        "decode_workers={}\nclip_threads={}\nbatch_size={}\nclip_execution_provider={}\nmax_file_size_mib={}\n",
        settings.decode_workers,
        settings.clip_threads,
        settings.batch_size,
        settings.clip_execution_provider.config_value(),
        settings.max_file_size_mib,
    );
    std::fs::write(path, content)
        .with_context(|| format!("writing performance settings {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-{label}-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn sanitized_settings_stay_inside_safe_ranges() {
        let settings = IndexingSettings {
            decode_workers: usize::MAX,
            clip_threads: 0,
            batch_size: usize::MAX,
            clip_execution_provider: ClipExecutionProvider::DirectMl,
            max_file_size_mib: usize::MAX,
        }
        .sanitized();

        assert_eq!(settings.decode_workers, max_decode_workers());
        assert_eq!(settings.clip_threads, 1);
        assert_eq!(settings.batch_size, MAX_BATCH_SIZE);
        assert_eq!(settings.max_file_size_mib, MAX_FILE_SIZE_MIB);
        assert_eq!(
            settings.clip_execution_provider,
            ClipExecutionProvider::DirectMl
        );
    }

    #[test]
    fn saved_settings_round_trip() {
        let path = temp_settings_path("round-trip");
        let expected = IndexingSettings {
            decode_workers: max_decode_workers().min(3),
            clip_threads: max_clip_threads().min(4),
            batch_size: 48,
            clip_execution_provider: ClipExecutionProvider::DirectMl,
            max_file_size_mib: 512,
        }
        .sanitized();

        save(&path, expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_values_are_clamped_and_invalid_provider_keeps_default() {
        let path = temp_settings_path("invalid");
        std::fs::write(
            &path,
            "decode_workers=0\nclip_threads=999999\nbatch_size=0\nclip_execution_provider=unknown\nmax_file_size_mib=0\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.decode_workers, 1);
        assert_eq!(loaded.clip_threads, max_clip_threads());
        assert_eq!(loaded.batch_size, 1);
        assert_eq!(loaded.max_file_size_mib, 1);
        assert_eq!(loaded.clip_execution_provider, ClipExecutionProvider::Cpu);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_size_setting_uses_default_limit() {
        let path = temp_settings_path("legacy-default");
        std::fs::write(&path, "batch_size=16\n").unwrap();
        assert_eq!(load(&path).max_file_size_mib, DEFAULT_MAX_FILE_SIZE_MIB);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_size_limit_is_inclusive_and_extension_agnostic() {
        let settings = IndexingSettings {
            max_file_size_mib: 256,
            ..IndexingSettings::default()
        };
        let limit = 256_u64 * 1024 * 1024;
        assert!(settings.allows_file_size(limit));
        assert!(!settings.allows_file_size(limit + 1));
    }

    #[test]
    fn provider_aliases_parse_for_backward_compatible_manual_edits() {
        assert_eq!(
            ClipExecutionProvider::parse("directml"),
            Some(ClipExecutionProvider::DirectMl)
        );
        assert_eq!(
            ClipExecutionProvider::parse("GPU"),
            Some(ClipExecutionProvider::DirectMl)
        );
        assert_eq!(
            ClipExecutionProvider::parse("cpu"),
            Some(ClipExecutionProvider::Cpu)
        );
    }
}
