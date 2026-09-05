from pathlib import Path

path = Path("src/ui/settings_window.rs")
text = path.read_text(encoding="utf-8")

open_updates = '''pub(super) fn open_updates(app: &mut ImageSearchApp, ctx: &egui::Context) {
    app.settings_open = true;
    let category_id = egui::Id::new("preferences-category");
    ctx.data_mut(|data| data.insert_temp(category_id, SettingsCategory::Updates.index()));
}

'''
show_marker = "pub(super) fn show(app: &mut ImageSearchApp, ctx: &egui::Context) {\n"
if open_updates not in text:
    if show_marker not in text:
        raise SystemExit("settings show marker not found")
    text = text.replace(show_marker, open_updates + show_marker, 1)

old_guard = "            let can_install = !app.busy && !app.face_model_download_running();\n"
new_guard = "            let can_install = !super::update_ui::install_blocked(app);\n"
if new_guard not in text:
    if old_guard not in text:
        raise SystemExit("update install guard target not found")
    text = text.replace(old_guard, new_guard, 1)

old_help = '                    "Finish active indexing/search/model work before installing"\n'
new_help = '                    "Finish active indexing/search/model/People work before installing"\n'
if new_help not in text:
    if old_help not in text:
        raise SystemExit("update install help target not found")
    text = text.replace(old_help, new_help, 1)

path.write_text(text, encoding="utf-8")
