from pathlib import Path

path = Path('src/ui/mod.rs')
text = path.read_text(encoding='utf-8')

if 'mod settings_window;' not in text:
    marker = 'mod face_search_panel;\n'
    if marker not in text:
        raise SystemExit('module insertion marker not found')
    text = text.replace(marker, marker + 'mod settings_window;\n', 1)

start_marker = '    fn show_settings_window(&mut self, ctx: &egui::Context) {\n'
end_marker = '    fn show_search_sidebar(&mut self, ctx: &egui::Context) {\n'
start = text.find(start_marker)
end = text.find(end_marker, start)
if start < 0 or end < 0:
    raise SystemExit('settings window span not found')

replacement = '''    fn show_settings_window(&mut self, ctx: &egui::Context) {\n        settings_window::show(self, ctx);\n    }\n\n'''
text = text[:start] + replacement + text[end:]
path.write_text(text, encoding='utf-8')
