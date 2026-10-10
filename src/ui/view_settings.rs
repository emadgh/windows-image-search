use super::{AppearanceMode, ImageSearchApp, SortMode, ThumbnailFit, ViewMode};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ViewSettings {
    pub inspector_open: bool,
    pub thumb_size: f32,
    pub thumb_fit: ThumbnailFit,
    pub view_mode: ViewMode,
    pub sort_mode: SortMode,
    pub appearance_mode: AppearanceMode,
    pub sidebar_width: f32,
    pub inspector_width: f32,
    pub window_size: [f32; 2],
    pub maximized: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            inspector_open: true,
            thumb_size: 168.0,
            thumb_fit: ThumbnailFit::Contain,
            view_mode: ViewMode::Grid,
            sort_mode: SortMode::Relevance,
            appearance_mode: AppearanceMode::System,
            sidebar_width: super::theme::SEARCH_SIDEBAR_DEFAULT,
            inspector_width: super::theme::INSPECTOR_DEFAULT,
            window_size: [1380.0, 860.0],
            maximized: false,
        }
    }
}

impl ViewSettings {
    pub fn path_for_db(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("view-settings.json")
    }

    fn sanitized(mut self) -> Self {
        fn bounded(value: f32, min: f32, max: f32, default: f32) -> f32 {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                default
            }
        }
        self.thumb_size = bounded(self.thumb_size, 96.0, 512.0, 168.0);
        self.sidebar_width = bounded(
            self.sidebar_width,
            super::theme::SEARCH_SIDEBAR_MIN,
            super::theme::SEARCH_SIDEBAR_MAX,
            super::theme::SEARCH_SIDEBAR_DEFAULT,
        );
        self.inspector_width = bounded(
            self.inspector_width,
            super::theme::INSPECTOR_MIN,
            super::theme::INSPECTOR_MAX,
            super::theme::INSPECTOR_DEFAULT,
        );
        self.window_size[0] = bounded(self.window_size[0], 900.0, 10000.0, 1380.0);
        self.window_size[1] = bounded(self.window_size[1], 600.0, 10000.0, 860.0);
        self
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default()
            .sanitized()
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pending = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(
            &pending,
            serde_json::to_vec_pretty(&self.clone().sanitized())?,
        )?;
        std::fs::rename(pending, path)?;
        Ok(())
    }
}

pub(super) struct ViewPreferences {
    pub settings: ViewSettings,
    saved: ViewSettings,
    observed: ViewSettings,
    path: PathBuf,
    due: Option<Instant>,
}

impl ViewPreferences {
    pub fn new(path: PathBuf) -> Self {
        let settings = ViewSettings::load(&path);
        Self {
            saved: settings.clone(),
            observed: settings.clone(),
            settings,
            path,
            due: None,
        }
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        if self.settings != self.saved {
            self.settings.save(&self.path)?;
            self.saved = self.settings.clone();
        }
        self.due = None;
        Ok(())
    }
}

impl ImageSearchApp {
    fn capture_view_settings(&mut self, ctx: Option<&egui::Context>) {
        let mut settings = self.view_preferences.settings.clone();
        settings.inspector_open = self.inspector_open;
        settings.thumb_size = self.thumb_size;
        settings.thumb_fit = self.thumb_fit;
        settings.view_mode = self.view_mode;
        settings.sort_mode = self.sort_mode;
        settings.appearance_mode = self.appearance_mode;
        if let Some(ctx) = ctx {
            ctx.input(|input| {
                let viewport = input.viewport();
                if viewport.minimized != Some(true) && viewport.fullscreen != Some(true) {
                    if let Some(maximized) = viewport.maximized {
                        settings.maximized = maximized;
                    }
                }
                if !settings.maximized
                    && viewport.minimized != Some(true)
                    && viewport.fullscreen != Some(true)
                {
                    if let Some(rect) = viewport.inner_rect {
                        settings.window_size = [rect.width().round(), rect.height().round()];
                    }
                }
            });
        }
        settings = settings.sanitized();
        if settings != self.view_preferences.observed {
            self.view_preferences.observed = settings.clone();
            self.view_preferences.settings = settings;
            self.view_preferences.due = Some(Instant::now() + Duration::from_millis(500));
        }
        if self.view_preferences.settings == self.view_preferences.saved {
            self.view_preferences.due = None;
        }
    }

    pub(super) fn persist_view_settings(&mut self, ctx: &egui::Context) {
        self.capture_view_settings(Some(ctx));
        if let Some(due) = self.view_preferences.due {
            if Instant::now() >= due {
                if let Err(error) = self.view_preferences.flush() {
                    self.last_error = Some(format!("Cannot save view preferences: {error:#}"));
                    self.view_preferences.due = Some(Instant::now() + Duration::from_secs(2));
                }
            }
            if let Some(due) = self.view_preferences.due {
                ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
            }
        }
    }
}

impl Drop for ImageSearchApp {
    fn drop(&mut self) {
        self.capture_view_settings(None);
        if let Err(error) = self.view_preferences.flush() {
            eprintln!("Cannot save view preferences: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_app(dir: &Path) -> ImageSearchApp {
        crate::update::save_settings(
            &dir.join("update-settings.ini"),
            crate::update::UpdateSettings {
                auto_check: false,
                auto_download: false,
            },
        )
        .unwrap();
        let mut app = ImageSearchApp::new(dir.join("index.sqlite3"), dir.join("models"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.busy && Instant::now() < deadline {
            app.process_startup_messages();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !app.busy,
            "test startup did not finish: {:?}",
            app.last_error
        );
        app
    }

    #[test]
    fn shutdown_flushes_last_changes_and_new_app_restores_view_state() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("wis-view-restart-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = test_app(&dir);
        app.inspector_open = false;
        app.thumb_size = 335.0;
        app.thumb_fit = ThumbnailFit::Cover;
        app.view_mode = ViewMode::Details;
        app.sort_mode = SortMode::Modified;
        app.appearance_mode = AppearanceMode::Light;
        app.view_preferences.settings.sidebar_width = 390.0;
        app.view_preferences.settings.inspector_width = 420.0;
        app.capture_view_settings(None);
        let first_due = app.view_preferences.due.unwrap();
        app.view_preferences.settings.sidebar_width = 400.0;
        app.capture_view_settings(None);
        assert!(app.view_preferences.due.unwrap() >= first_due);
        assert!(
            !dir.join("view-settings.json").exists(),
            "changes should be debounced until saved or closed"
        );
        drop(app);
        let app = test_app(&dir);
        assert!(!app.inspector_open);
        assert_eq!(app.thumb_size, 335.0);
        assert_eq!(app.thumb_fit, ThumbnailFit::Cover);
        assert_eq!(app.view_mode, ViewMode::Details);
        assert_eq!(app.sort_mode, SortMode::Modified);
        assert_eq!(app.appearance_mode, AppearanceMode::Light);
        assert_eq!(app.view_preferences.settings.sidebar_width, 400.0);
        assert_eq!(app.view_preferences.settings.inspector_width, 420.0);
        drop(app);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => panic!("cleanup failed: {error}"),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn minimized_or_maximized_window_keeps_the_normal_restore_size() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("wis-view-window-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = test_app(&dir);
        app.view_preferences.settings.window_size = [1600.0, 900.0];
        app.view_preferences.settings.maximized = true;
        let ctx = egui::Context::default();
        for (minimized, maximized) in [(true, false), (false, true)] {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            viewport.minimized = Some(minimized);
            viewport.maximized = Some(maximized);
            viewport.inner_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1920.0, 1080.0),
            ));
            let _ = ctx.run(input, |ctx| app.capture_view_settings(Some(ctx)));
            assert_eq!(app.view_preferences.settings.window_size, [1600.0, 900.0]);
            assert!(app.view_preferences.settings.maximized);
        }
        drop(app);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => panic!("cleanup failed: {error}"),
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn all_view_preferences_round_trip_and_replace_previous_save() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("wis-view-{}-{nonce}.json", std::process::id()));
        let settings = ViewSettings {
            inspector_open: false,
            thumb_size: 335.0,
            thumb_fit: ThumbnailFit::Cover,
            view_mode: ViewMode::Details,
            sort_mode: SortMode::Name,
            appearance_mode: AppearanceMode::Dark,
            sidebar_width: 400.0,
            inspector_width: 450.0,
            window_size: [1600.0, 900.0],
            maximized: true,
        };
        ViewSettings::default().save(&path).unwrap();
        settings.save(&path).unwrap();
        assert_eq!(ViewSettings::load(&path), settings.sanitized());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn partial_preferences_keep_defaults_and_invalid_sizes_are_clamped() {
        let settings: ViewSettings = serde_json::from_str(
            r#"{"inspector_open":false,"thumb_size":99999,"sidebar_width":0}"#,
        )
        .unwrap();
        let settings = settings.sanitized();
        assert!(!settings.inspector_open);
        assert_eq!(settings.thumb_size, 512.0);
        assert_eq!(
            settings.sidebar_width,
            super::super::theme::SEARCH_SIDEBAR_MIN
        );
        assert_eq!(settings.thumb_fit, ThumbnailFit::Contain);
    }
}
