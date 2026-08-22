use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeopleSettings {
    pub similarity_threshold: f32,
    pub min_cluster_size: usize,
}

impl Default for PeopleSettings {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.62,
            min_cluster_size: 2,
        }
    }
}

impl PeopleSettings {
    pub fn sanitized(self) -> Self {
        Self {
            similarity_threshold: if self.similarity_threshold.is_finite() {
                self.similarity_threshold.clamp(0.0, 1.0)
            } else {
                Self::default().similarity_threshold
            },
            min_cluster_size: self.min_cluster_size.clamp(2, 1_000_000),
        }
    }
}

pub fn load(path: &Path) -> PeopleSettings {
    let mut settings = PeopleSettings::default();
    let Ok(content) = std::fs::read_to_string(path) else {
        return settings;
    };

    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "similarity_threshold" => {
                if let Ok(parsed) = value.trim().parse::<f32>() {
                    settings.similarity_threshold = parsed;
                }
            }
            "min_cluster_size" => {
                if let Ok(parsed) = value.trim().parse::<usize>() {
                    settings.min_cluster_size = parsed;
                }
            }
            _ => {}
        }
    }
    settings.sanitized()
}

pub fn save(path: &Path, settings: &PeopleSettings) -> Result<()> {
    let settings = settings.sanitized();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating People settings directory {}", parent.display()))?;
    }
    std::fs::write(
        path,
        format!(
            "similarity_threshold={:.4}\nmin_cluster_size={}\n",
            settings.similarity_threshold, settings.min_cluster_size
        ),
    )
    .with_context(|| format!("saving People settings {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings_path() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "windows-image-search-people-settings-{}-{unique}",
                std::process::id()
            ))
            .join("people-settings.ini")
    }

    #[test]
    fn invalid_values_are_sanitized() {
        assert_eq!(
            PeopleSettings {
                similarity_threshold: f32::NAN,
                min_cluster_size: 0,
            }
            .sanitized(),
            PeopleSettings::default()
        );
        assert_eq!(
            PeopleSettings {
                similarity_threshold: 9.0,
                min_cluster_size: 1,
            }
            .sanitized(),
            PeopleSettings {
                similarity_threshold: 1.0,
                min_cluster_size: 2,
            }
        );
    }

    #[test]
    fn settings_round_trip() {
        let path = temp_settings_path();
        let expected = PeopleSettings {
            similarity_threshold: 0.71,
            min_cluster_size: 3,
        };
        save(&path, &expected).unwrap();
        assert_eq!(load(&path), expected);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
