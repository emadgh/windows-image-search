use std::path::Path;

use update_via_github::UpdateConfig;
pub use update_via_github::UpdateStatus;

const REPOSITORY: &str = "emadgh/windows-image-search";
const ASSET_NAME: &str = "windows-image-search.exe";
const CHECKSUM_ASSET_NAME: &str = "windows-image-search.exe.sha256";
const MAX_DOWNLOAD_SIZE: usize = 256 * 1024 * 1024;
const MIN_EXECUTABLE_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateSettings {
    pub auto_check: bool,
    pub auto_download: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: true,
        }
    }
}

pub fn load_settings(path: &Path) -> UpdateSettings {
    let mut settings = UpdateSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let parsed = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
        match key.trim() {
            "auto_check" => settings.auto_check = parsed,
            "auto_download" => settings.auto_download = parsed,
            _ => {}
        }
    }
    settings
}

pub fn save_settings(path: &Path, settings: UpdateSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        format!(
            "auto_check={}\nauto_download={}\n",
            settings.auto_check, settings.auto_download
        ),
    )
}

#[derive(Clone)]
pub struct UpdateManager {
    inner: update_via_github::UpdateManager,
}

impl Default for UpdateManager {
    fn default() -> Self {
        let config = UpdateConfig::new(REPOSITORY, ASSET_NAME, env!("CARGO_PKG_VERSION"))
            .with_app_name("WindowsImageSearch")
            .with_checksum_asset(CHECKSUM_ASSET_NAME)
            .with_required_checksum(true)
            .with_max_download_size(MAX_DOWNLOAD_SIZE)
            .with_min_executable_size(MIN_EXECUTABLE_SIZE);
        Self {
            inner: update_via_github::UpdateManager::new(config),
        }
    }
}

impl UpdateManager {
    pub fn status(&self) -> UpdateStatus {
        self.inner.status()
    }

    pub fn start_check(&self, auto_download: bool) -> bool {
        self.inner.start_check(auto_download)
    }

    pub fn start_download(&self) -> bool {
        self.inner.start_download()
    }

    pub fn apply_ready(&self) -> Result<bool, String> {
        self.inner.apply_ready()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_settings_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "windows-image-search-update-settings-{}-{nonce}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn update_settings_default_to_background_check_and_download() {
        assert_eq!(
            UpdateSettings::default(),
            UpdateSettings {
                auto_check: true,
                auto_download: true,
            }
        );
    }

    #[test]
    fn update_settings_round_trip() {
        let path = temp_settings_path();
        let expected = UpdateSettings {
            auto_check: false,
            auto_download: true,
        };

        save_settings(&path, expected).expect("save update settings");
        assert_eq!(load_settings(&path), expected);

        let _ = std::fs::remove_file(path);
    }
}
