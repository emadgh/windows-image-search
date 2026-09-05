from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


def replace_method(text: str, signature: str, replacement: str, label: str) -> str:
    start = text.find(signature)
    if start < 0:
        raise SystemExit(f"missing method: {label}")
    brace = text.find("{", start)
    if brace < 0:
        raise SystemExit(f"missing method brace: {label}")
    depth = 0
    for i in range(brace, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[:start] + replacement + text[i + 1 :]
    raise SystemExit(f"unbalanced method: {label}")


# --- theme module -------------------------------------------------------------
Path("src/ui/theme.rs").write_text(r'''use super::{AppearanceMode, ImageSearchApp};
use eframe::egui;

pub(super) const SEARCH_SIDEBAR_DEFAULT: f32 = 310.0;
pub(super) const SEARCH_SIDEBAR_MIN: f32 = 270.0;
pub(super) const SEARCH_SIDEBAR_MAX: f32 = 430.0;
pub(super) const INSPECTOR_DEFAULT: f32 = 320.0;
pub(super) const INSPECTOR_MIN: f32 = 260.0;
pub(super) const INSPECTOR_MAX: f32 = 430.0;
pub(super) const TOP_BAR_HEIGHT: f32 = 38.0;
pub(super) const STATUS_BAR_HEIGHT: f32 = 34.0;

impl ImageSearchApp {
    pub(super) fn apply_design_system(&mut self, ctx: &egui::Context) {
        let inherited_dark = ctx.style().visuals.dark_mode;
        let system_dark = *self.system_dark_mode.get_or_insert(inherited_dark);
        let dark = match self.appearance_mode {
            AppearanceMode::System => system_dark,
            AppearanceMode::Light => false,
            AppearanceMode::Dark => true,
        };

        let mut style = (*ctx.style()).clone();
        style.visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.indent = 16.0;
        style.spacing.interact_size.y = 28.0;
        style.spacing.icon_spacing = 6.0;

        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(18.0),
        );
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));

        let control_radius = egui::CornerRadius::same(6);
        style.visuals.window_corner_radius = egui::CornerRadius::same(8);
        style.visuals.menu_corner_radius = control_radius;
        style.visuals.widgets.noninteractive.corner_radius = control_radius;
        style.visuals.widgets.inactive.corner_radius = control_radius;
        style.visuals.widgets.hovered.corner_radius = control_radius;
        style.visuals.widgets.active.corner_radius = control_radius;
        style.visuals.widgets.open.corner_radius = control_radius;
        style.visuals.selection.stroke.width = 2.0;

        ctx.set_style(style);
    }
}
''', encoding="utf-8")


# --- task center --------------------------------------------------------------
Path("src/ui/task_center.rs").write_text(r'''use super::ImageSearchApp;
use eframe::egui;

impl ImageSearchApp {
    fn has_background_activity(&self) -> bool {
        self.busy
            || self.indexing
            || self.searching
            || self.face_model_download_running()
            || !self.pending_fs_paths.is_empty()
    }

    pub(super) fn show_task_status_button(&mut self, ui: &mut egui::Ui) {
        let active = self.has_background_activity();
        let label = if active { "Tasks · Active" } else { "Tasks" };
        if ui.selectable_label(self.task_center_open, label).clicked() {
            self.task_center_open = !self.task_center_open;
        }
    }

    pub(super) fn show_task_center(&mut self, ctx: &egui::Context) {
        if !self.task_center_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.task_center_open = false;
            return;
        }

        let mut open = self.task_center_open;
        egui::Window::new("Task Center")
            .open(&mut open)
            .resizable(true)
            .default_size([520.0, 330.0])
            .min_width(400.0)
            .show(ctx, |ui| {
                ui.heading("Background activity");
                ui.small("Indexing, search, model downloads and filesystem work are consolidated here.");
                ui.add_space(8.0);

                if self.has_background_activity() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let title = if self.indexing {
                                if self.index_paused { "Indexing paused" } else { "Indexing library" }
                            } else if self.searching {
                                "Searching"
                            } else if self.face_model_download_running() {
                                "Downloading face model"
                            } else {
                                "Background work"
                            };
                            ui.strong(title);
                        });
                        ui.label(super::views::truncate_middle(&self.status, 120))
                            .on_hover_text(&self.status);

                        if let Some((done, total)) = self.progress.filter(|(_, total)| *total > 0) {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(ui.available_width().min(440.0))
                                    .text(format!("{done}/{total}")),
                            );
                        }
                        if let Some(file_name) = &self.current_file {
                            ui.small(format!("Current: {file_name}"));
                        }
                        if self.indexing && self.index_control.is_some() {
                            let label = if self.index_paused { "Resume indexing" } else { "Pause indexing" };
                            if ui
                                .add_enabled(!self.searching, egui::Button::new(label))
                                .clicked()
                            {
                                self.toggle_index_pause();
                            }
                        }
                    });
                } else {
                    ui.group(|ui| {
                        ui.strong("No active tasks");
                        ui.small("Background indexing and search activity will appear here when it starts.");
                    });
                }

                if !self.pending_fs_paths.is_empty() {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Filesystem queue");
                        ui.label(format!(
                            "{} changed path{} waiting for the current operation to finish.",
                            self.pending_fs_paths.len(),
                            if self.pending_fs_paths.len() == 1 { " is" } else { "s are" }
                        ));
                    });
                }

                if let Some(reason) = &self.watcher_reconcile_required {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Reconciliation recommended");
                        ui.label(reason);
                    });
                }

                if let Some(error) = &self.last_error {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.strong("Needs attention");
                        ui.label(error);
                    });
                }
            });
        self.task_center_open = open;
    }
}
''', encoding="utf-8")


# --- main UI state / rendering ------------------------------------------------
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "mod settings_window;\n", "mod settings_window;\nmod task_center;\nmod theme;\n", "module declarations")
text = once(
    text,
    "#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum SearchMode {\n",
    "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub(super) enum AppearanceMode {\n    System,\n    Light,\n    Dark,\n}\n\nimpl AppearanceMode {\n    pub(super) fn label(self) -> &'static str {\n        match self {\n            Self::System => \"System\",\n            Self::Light => \"Light\",\n            Self::Dark => \"Dark\",\n        }\n    }\n}\n\n#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum SearchMode {\n",
    "appearance enum",
)
text = once(
    text,
    "    pub(super) inspector_open: bool,\n",
    "    pub(super) inspector_open: bool,\n    pub(super) appearance_mode: AppearanceMode,\n    pub(super) system_dark_mode: Option<bool>,\n    pub(super) task_center_open: bool,\n",
    "appearance/task fields",
)
text = once(
    text,
    "            inspector_open: true,\n",
    "            inspector_open: true,\n            appearance_mode: AppearanceMode::System,\n            system_dark_mode: None,\n            task_center_open: false,\n",
    "appearance/task defaults",
)
text = once(
    text,
    "    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {\n        self.process_startup_messages();\n",
    "    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {\n        self.apply_design_system(ctx);\n        self.process_startup_messages();\n",
    "apply design system",
)
text = once(
    text,
    "        self.show_close_confirmation(ctx);\n        self.show_error_banner(ctx);\n",
    "        self.show_close_confirmation(ctx);\n        self.show_error_banner(ctx);\n        self.show_task_center(ctx);\n",
    "task center render",
)
text = once(
    text,
    "        egui::TopBottomPanel::top(\"top\").show(ctx, |ui| {\n            ui.horizontal(|ui| {\n",
    "        egui::TopBottomPanel::top(\"top\").show(ctx, |ui| {\n            ui.set_min_height(theme::TOP_BAR_HEIGHT);\n            ui.horizontal(|ui| {\n",
    "top bar height",
)
text = once(
    text,
    "        egui::TopBottomPanel::bottom(\"status\").show(ctx, |ui| {\n            ui.horizontal(|ui| {\n",
    "        egui::TopBottomPanel::bottom(\"status\").show(ctx, |ui| {\n            ui.set_min_height(theme::STATUS_BAR_HEIGHT);\n            ui.horizontal(|ui| {\n",
    "status bar height",
)
text = once(
    text,
    "                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                    if self.indexing && self.index_control.is_some() {\n",
    "                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {\n                    self.show_task_status_button(ui);\n                    ui.separator();\n                    if self.indexing && self.index_control.is_some() {\n",
    "task center status entry",
)
text = text.replace('"▶ Resume"', '"Resume"').replace('"⏸ Pause"', '"Pause"')
path.write_text(text, encoding="utf-8")


# --- floating error feedback + shortcut guard ---------------------------------
path = Path("src/ui/ux.rs")
text = path.read_text(encoding="utf-8")
text = once(
    text,
    "            || self.collections_open\n            || self.close_confirmation_open\n",
    "            || self.collections_open\n            || self.task_center_open\n            || self.close_confirmation_open\n",
    "task center shortcut guard",
)
text = replace_method(
    text,
    "    pub(super) fn show_error_banner(&mut self, ctx: &egui::Context)",
    r'''    pub(super) fn show_error_banner(&mut self, ctx: &egui::Context) {
        let Some(error) = self.last_error.clone() else {
            return;
        };
        let mut dismiss = false;
        let mut open_tasks = false;
        egui::Area::new(egui::Id::new("global-error-toast"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 50.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.strong("Needs attention");
                    ui.label(super::views::truncate_middle(&error, 180))
                        .on_hover_text(&error);
                    ui.horizontal(|ui| {
                        if ui.small_button("Task Center").clicked() {
                            open_tasks = true;
                        }
                        if ui.small_button("Dismiss").clicked() {
                            dismiss = true;
                        }
                    });
                });
            });
        if open_tasks {
            self.task_center_open = true;
        }
        if dismiss {
            self.last_error = None;
        }
    }''',
    "error toast",
)
path.write_text(text, encoding="utf-8")


# --- consistent panel geometry -------------------------------------------------
path = Path("src/ui/search_panel.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::{ImageSearchApp, SearchMode};\n", "use super::{theme, ImageSearchApp, SearchMode};\n", "search theme import")
text = text.replace(".default_width(310.0)", ".default_width(theme::SEARCH_SIDEBAR_DEFAULT)", 1)
text = text.replace(".min_width(270.0)", ".min_width(theme::SEARCH_SIDEBAR_MIN)", 1)
text = text.replace(".max_width(430.0)", ".max_width(theme::SEARCH_SIDEBAR_MAX)", 1)
path.write_text(text, encoding="utf-8")

path = Path("src/ui/inspector.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::ImageSearchApp;\n", "use super::{theme, ImageSearchApp};\n", "inspector theme import")
text = text.replace(".default_width(320.0)", ".default_width(theme::INSPECTOR_DEFAULT)", 1)
text = text.replace(".min_width(260.0)", ".min_width(theme::INSPECTOR_MIN)", 1)
text = text.replace(".max_width(430.0)", ".max_width(theme::INSPECTOR_MAX)", 1)
path.write_text(text, encoding="utf-8")


# --- Appearance settings -------------------------------------------------------
path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "use super::ImageSearchApp;\n", "use super::{AppearanceMode, ImageSearchApp};\n", "settings appearance import")
text = once(
    text,
    "enum SettingsCategory {\n    SearchClip,\n",
    "enum SettingsCategory {\n    Appearance,\n    SearchClip,\n",
    "appearance category enum",
)
text = once(
    text,
    "    const ALL: [Self; 4] = [\n        Self::SearchClip,\n",
    "    const ALL: [Self; 5] = [\n        Self::Appearance,\n        Self::SearchClip,\n",
    "appearance category list",
)
text = once(
    text,
    "        match self {\n            Self::SearchClip => \"Search / CLIP\",\n",
    "        match self {\n            Self::Appearance => \"Appearance\",\n            Self::SearchClip => \"Search / CLIP\",\n",
    "appearance category label",
)
text = once(
    text,
    "        match self {\n            Self::SearchClip => 0,\n            Self::FacesPeople => 1,\n            Self::Performance => 2,\n            Self::Storage => 3,\n",
    "        match self {\n            Self::Appearance => 0,\n            Self::SearchClip => 1,\n            Self::FacesPeople => 2,\n            Self::Performance => 3,\n            Self::Storage => 4,\n",
    "appearance category index",
)
text = once(
    text,
    "        match index {\n            1 => Self::FacesPeople,\n            2 => Self::Performance,\n            3 => Self::Storage,\n            _ => Self::SearchClip,\n",
    "        match index {\n            1 => Self::SearchClip,\n            2 => Self::FacesPeople,\n            3 => Self::Performance,\n            4 => Self::Storage,\n            _ => Self::Appearance,\n",
    "appearance category decode",
)
text = once(
    text,
    "                                .show(ui, |ui| match category {\n                                    SettingsCategory::SearchClip => {\n",
    "                                .show(ui, |ui| match category {\n                                    SettingsCategory::Appearance => settings_appearance(app, ui),\n                                    SettingsCategory::SearchClip => {\n",
    "appearance category renderer",
)
marker = "fn settings_search_clip(app: &mut ImageSearchApp, ui: &mut egui::Ui, effects: &mut Effects) {\n"
appearance_fn = r'''fn settings_appearance(app: &mut ImageSearchApp, ui: &mut egui::Ui) {
    section_title(
        ui,
        "Appearance",
        "Choose a compact application theme. System follows the appearance captured when the app starts.",
    );

    ui.horizontal_wrapped(|ui| {
        for mode in [AppearanceMode::System, AppearanceMode::Light, AppearanceMode::Dark] {
            if ui
                .selectable_label(app.appearance_mode == mode, mode.label())
                .clicked()
            {
                app.appearance_mode = mode;
            }
        }
    });
    ui.add_space(8.0);
    ui.small("The same spacing, typography, selection treatment and control geometry are used in both light and dark themes.");
}

'''
text = once(text, marker, appearance_fn + marker, "appearance settings function")
path.write_text(text, encoding="utf-8")


# --- remove mixed transport/refresh emoji labels in UI strings ----------------
for file in Path("src/ui").glob("*.rs"):
    data = file.read_text(encoding="utf-8")
    data = data.replace("⟳ ", "").replace("＋ ", "").replace("▶ ", "").replace("⏸ ", "")
    file.write_text(data, encoding="utf-8")
