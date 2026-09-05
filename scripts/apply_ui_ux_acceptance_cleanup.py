from pathlib import Path


def once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise SystemExit(f"missing patch target: {label}")
    return text.replace(old, new, 1)


# #201: concise summary remains visible while Advanced similarity is collapsed.
path = Path("src/ui/search_panel.rs")
text = path.read_text(encoding="utf-8")
old = '''                            if self.query_image.is_some() {
                                ui.add_space(8.0);
                                egui::CollapsingHeader::new("Advanced similarity")
'''
new = '''                            if self.query_image.is_some() {
                                ui.add_space(8.0);
                                ui.small(format!(
                                    "Mix · Color {:.0}% · Texture {:.0}% · Semantic {:.0}% · Dominant {:.0}%{}",
                                    self.similarity_settings.color_distribution_weight,
                                    self.similarity_settings.texture_weight,
                                    self.similarity_settings.clip_weight,
                                    self.similarity_settings.dominant_color_weight,
                                    if self.similarity_settings.strict_color_rejection {
                                        " · strict color"
                                    } else {
                                        ""
                                    }
                                ));
                                egui::CollapsingHeader::new("Advanced similarity")
'''
text = once(text, old, new, "similarity collapsed summary")
path.write_text(text, encoding="utf-8")


# #199/#205: Show in Explorer selects the actual file on Windows.
path = Path("src/windows_shell.rs")
text = path.read_text(encoding="utf-8")
insert = '''use std::path::PathBuf;

pub fn show_in_explorer(path: PathBuf) {
    #[cfg(target_os = "windows")]
    {
        let argument = format!("/select,{}", path.display());
        if let Err(err) = std::process::Command::new("explorer.exe").arg(argument).spawn() {
            eprintln!("Cannot show {} in Explorer: {err}", path.display());
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(parent) = path.parent() {
            let _ = open::that(parent);
        }
    }
}

'''
text = once(text, "use std::path::PathBuf;\n\n", insert, "show in explorer helper")
path.write_text(text, encoding="utf-8")

path = Path("src/ui/inspector.rs")
text = path.read_text(encoding="utf-8")
old = '''                    if ui.button("Open folder").clicked() {
                        if let Some(parent) = record.path.parent() {
                            let _ = open::that(parent);
                        }
                    }
'''
new = '''                    if ui.button("Show in Explorer").clicked() {
                        crate::windows_shell::show_in_explorer(record.path.clone());
                    }
'''
text = once(text, old, new, "inspector show in explorer")
path.write_text(text, encoding="utf-8")

path = Path("src/ui/ux.rs")
text = path.read_text(encoding="utf-8")
old = '''                if ui
                    .add_enabled(single.is_some(), egui::Button::new("Open folder"))
                    .clicked()
                {
                    if let Some(parent) = single.as_ref().and_then(|path| path.parent()) {
                        let _ = open::that(parent);
                    }
                }
'''
new = '''                if ui
                    .add_enabled(single.is_some(), egui::Button::new("Show in Explorer"))
                    .clicked()
                {
                    if let Some(path) = &single {
                        crate::windows_shell::show_in_explorer(path.clone());
                    }
                }
'''
text = once(text, old, new, "selection show in explorer")
path.write_text(text, encoding="utf-8")
