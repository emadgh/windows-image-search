use anyhow::{Context, Result};
use std::path::Path;

pub const MAX_BATCH_SIZE: usize = 256;
const MAX_CONFIGURED_THREADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexingSettings {
    pub decode_workers: usize,
    pub clip_threads: usize,
    pub batch_size: usize,
}

impl Default for IndexingSettings {
    fn default() -> Self {
        let logical = logical_parallelism();
        Self {
            decode_workers: logical.saturating_sub(1).max(1).min(2),
            clip_threads: logical.saturating_sub(1).max(1).min(4),
            batch_size: 16,
        }
    }
}

impl IndexingSettings {
    pub fn sanitized(self) -> Self {
        Self {
            decode_workers: self.decode_workers.clamp(1, max_decode_workers()),
            clip_threads: self.clip_threads.clamp(1, max_clip_threads()),
            batch_size: self.batch_size.clamp(1, MAX_BATCH_SIZE),
        }
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
        let Ok(value) = value.trim().parse::<usize>() else {
            continue;
        };
        match key.trim() {
            "decode_workers" => settings.decode_workers = value,
            "clip_threads" => settings.clip_threads = value,
            "batch_size" => settings.batch_size = value,
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
        "decode_workers={}\nclip_threads={}\nbatch_size={}\n",
        settings.decode_workers, settings.clip_threads, settings.batch_size
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
        }
        .sanitized();

        assert_eq!(settings.decode_workers, max_decode_workers());
        assert_eq!(settings.clip_threads, 1);
        assert_eq!(settings.batch_size, MAX_BATCH_SIZE);
    }

    #[test]
    fn saved_settings_round_trip() {
        let path = temp_settings_path("round-trip");
        let expected = IndexingSettings {
            decode_workers: max_decode_workers().min(3),
            clip_threads: max_clip_threads().min(4),
            batch_size: 48,
        }
        .sanitized();

        save(&path, expected).unwrap();
        assert_eq!(load(&path), expected);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_values_are_clamped() {
        let path = temp_settings_path("invalid");
        std::fs::write(
            &path,
            "decode_workers=0\nclip_threads=999999\nbatch_size=0\nunknown=12\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.decode_workers, 1);
        assert_eq!(loaded.clip_threads, max_clip_threads());
        assert_eq!(loaded.batch_size, 1);
        let _ = std::fs::remove_file(path);
    }
}
