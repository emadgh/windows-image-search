from pathlib import Path

path = Path("src/ui/face_search_panel.rs")
text = path.read_text(encoding="utf-8")
old = '''    pub(super) fn show_face_search_sidebar(&mut self, ui: &mut egui::Ui) {
        if self.face_search_ui.suggestions.is_empty() && !self.face_search_ui.loading {
            self.refresh_face_suggestions();
        }

'''
new = '''    pub(super) fn show_face_search_sidebar(&mut self, ui: &mut egui::Ui) {
'''
if old not in text:
    raise SystemExit("inline face auto-refresh target not found")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
