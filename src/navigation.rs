//! Persistent navigation preferences and Windows-aware directory boundaries.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NavigationSettings {
    pub show_tree: bool,
    pub filter_on_click: bool,
    pub include_subfolders: bool,
    pub auto_follow: bool,
    pub highlight_folder: bool,
    pub auto_rerun: bool,
    pub tree_fraction: f32,
}

impl Default for NavigationSettings {
    fn default() -> Self {
        Self {
            show_tree: true,
            filter_on_click: true,
            include_subfolders: true,
            auto_follow: true,
            highlight_folder: true,
            auto_rerun: true,
            tree_fraction: 0.45,
        }
    }
}

impl NavigationSettings {
    pub fn load(path: &Path) -> Self {
        let mut value: Self = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        value.tree_fraction = if value.tree_fraction.is_finite() {
            value.tree_fraction.clamp(0.15, 0.8)
        } else {
            0.45
        };
        value
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

pub fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let text = if let Some(unc) = text.strip_prefix(r"\\?\unc\") {
        format!(r"\\{unc}")
    } else {
        text
    };
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .trim_end_matches('\\')
        .to_owned()
}

pub fn within(path: &Path, folder: &Path) -> bool {
    let path = path_key(path);
    let folder = path_key(folder);
    path == folder
        || path
            .strip_prefix(&folder)
            .is_some_and(|rest| rest.starts_with('\\'))
}

pub fn folder_matches(path: &Path, folder: Option<&PathBuf>, recursive: bool) -> bool {
    folder.is_none_or(|folder| {
        if recursive {
            within(path, folder)
        } else {
            path.parent()
                .is_some_and(|parent| path_key(parent) == path_key(folder))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_directory_boundaries_and_direct_children() {
        let folder = PathBuf::from(r"C:\Photos");
        assert!(within(Path::new(r"c:\PHOTOS\child\x.jpg"), &folder));
        assert!(!within(Path::new(r"C:\Photos2\x.jpg"), &folder));
        assert!(within(Path::new(r"C:\Photos\x.jpg"), Path::new(r"C:\")));
        assert!(within(Path::new(r"\\?\C:\Photos\x.jpg"), &folder));
        assert!(within(
            Path::new(r"\\?\UNC\server\share\child\x.jpg"),
            Path::new(r"\\server\share")
        ));
        assert!(folder_matches(
            Path::new(r"C:\Photos\x.jpg"),
            Some(&folder),
            false
        ));
        assert!(!folder_matches(
            Path::new(r"C:\Photos\child\x.jpg"),
            Some(&folder),
            false
        ));
    }
    #[test]
    fn preferences_round_trip_and_legacy_defaults() {
        let settings = NavigationSettings {
            auto_follow: false,
            include_subfolders: false,
            ..Default::default()
        };
        let path = std::env::temp_dir().join(format!("wis-navigation-{}.json", std::process::id()));
        settings.save(&path).unwrap();
        assert_eq!(NavigationSettings::load(&path), settings);
        std::fs::remove_file(path).unwrap();
        let old: NavigationSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, NavigationSettings::default());
    }
}
