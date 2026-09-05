from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# Standalone Collections workspace reuses the safe collection/index management UI.
Path("src/ui/collections_window.rs").write_text(
    '''use super::ImageSearchApp;
use eframe::egui;

impl ImageSearchApp {
    pub(super) fn show_collections_workspace(&mut self, ctx: &egui::Context) {
        if !self.collections_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.collections_open = false;
            return;
        }

        let mut open = self.collections_open;
        egui::Window::new("Collections")
            .open(&mut open)
            .resizable(true)
            .default_size([860.0, 680.0])
            .min_size([620.0, 460.0])
            .max_height((ctx.available_rect().height() - 48.0).max(360.0))
            .show(ctx, |ui| {
                ui.heading("Collections");
                ui.label(
                    "Organize indexed folders and images without moving or deleting source files.",
                );
                ui.add_space(6.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("collections-workspace-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.show_collections_settings(ui));
            });
        self.collections_open = open;
    }
}
''',
    encoding="utf-8",
)


# App state + main navigation.
path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
text = once(text, "mod collections;\n", "mod collections;\nmod collections_window;\n", "collections workspace module")
text = once(
    text,
    """    collections: collections::CollectionsState,\n    pub(super) search_text: String,\n""",
    """    collections: collections::CollectionsState,\n    pub(super) collections_open: bool,\n    pub(super) search_text: String,\n""",
    "collections window state",
)
text = once(
    text,
    """            collections: collections::CollectionsState::default(),\n            search_text: String::new(),\n""",
    """            collections: collections::CollectionsState::default(),\n            collections_open: false,\n            search_text: String::new(),\n""",
    "collections window default",
)
text = once(
    text,
    """                if ui\n                    .selectable_label(self.collection_filter_chip().is_none(), \"Library\")\n                    .clicked()\n                {\n                    self.clear_collection_filter();\n                }\n                if ui\n                    .button(format!(\"Collections ({})\", self.collection_count()))\n                    .clicked()\n                {\n                    settings_window::open_collections(self, ctx);\n                }\n""",
    """                if ui\n                    .selectable_label(self.collection_filter_chip().is_none(), \"All Photos\")\n                    .clicked()\n                {\n                    self.clear_collection_filter();\n                }\n                if ui\n                    .button(format!(\"Collections ({})\", self.collection_count()))\n                    .clicked()\n                {\n                    self.collections_open = true;\n                }\n""",
    "main library navigation",
)
text = once(
    text,
    """        self.show_search_sidebar(ctx);\n        self.show_inspector(ctx);\n        self.show_settings_window(ctx);\n""",
    """        self.show_search_sidebar(ctx);\n        self.show_inspector(ctx);\n        self.show_collections_workspace(ctx);\n        self.show_settings_window(ctx);\n""",
    "collections workspace render",
)
path.write_text(text, encoding="utf-8")


# Preferences now contain app preferences only, not collection/library management.
path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")
old_enum = '''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsCategory {
    Collections,
    SearchClip,
    FacesPeople,
    Performance,
    Storage,
}

impl SettingsCategory {
    const ALL: [Self; 5] = [
        Self::Collections,
        Self::SearchClip,
        Self::FacesPeople,
        Self::Performance,
        Self::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Collections => "Collections",
            Self::SearchClip => "Search / CLIP",
            Self::FacesPeople => "Faces / People",
            Self::Performance => "Performance",
            Self::Storage => "Storage",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::Collections => 0,
            Self::SearchClip => 1,
            Self::FacesPeople => 2,
            Self::Performance => 3,
            Self::Storage => 4,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::SearchClip,
            2 => Self::FacesPeople,
            3 => Self::Performance,
            4 => Self::Storage,
            _ => Self::Collections,
        }
    }
}

pub(super) fn open_collections(app: &mut ImageSearchApp, ctx: &egui::Context) {
    app.settings_open = true;
    let category_id = egui::Id::new("preferences-category");
    ctx.data_mut(|data| data.insert_temp(category_id, SettingsCategory::Collections.index()));
}
'''
new_enum = '''#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsCategory {
    SearchClip,
    FacesPeople,
    Performance,
    Storage,
}

impl SettingsCategory {
    const ALL: [Self; 4] = [
        Self::SearchClip,
        Self::FacesPeople,
        Self::Performance,
        Self::Storage,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SearchClip => "Search / CLIP",
            Self::FacesPeople => "Faces / People",
            Self::Performance => "Performance",
            Self::Storage => "Storage",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::SearchClip => 0,
            Self::FacesPeople => 1,
            Self::Performance => 2,
            Self::Storage => 3,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::FacesPeople,
            2 => Self::Performance,
            3 => Self::Storage,
            _ => Self::SearchClip,
        }
    }
}
'''
text = once(text, old_enum, new_enum, "preferences categories")
text = once(
    text,
    '''                                .show(ui, |ui| match category {
                                    SettingsCategory::Collections => settings_collections(app, ui),
                                    SettingsCategory::SearchClip => {
''',
    '''                                .show(ui, |ui| match category {
                                    SettingsCategory::SearchClip => {
''',
    "preferences category rendering",
)
old_fn = '''fn settings_collections(app: &mut ImageSearchApp, ui: &mut egui::Ui) {
    section_title(
        ui,
        "Collections / Indexing",
        "Collections are the library. Add folders here to attach/index them; source files and portable .imagesearch data are never deleted by collection edits.",
    );
    app.show_collections_settings(ui);
}

'''
text = once(text, old_fn, "", "remove collection preferences page")
path.write_text(text, encoding="utf-8")
