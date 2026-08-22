from pathlib import Path

path = Path('src/ui/collections.rs')
text = path.read_text(encoding='utf-8')
old = '''    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {\n        ui.add_space(12.0);\n        ui.separator();\n        ui.heading("Collections");\n        ui.label(\n            "Collections are virtual groups. Assign indexed folders recursively, add individual indexed files, or drag items here. Source files are never moved or deleted.",\n        );\n'''
new = '''    pub(super) fn show_collections_settings(&mut self, ui: &mut egui::Ui) {\n        ui.label(\n            "Collections are virtual groups. Assign indexed folders recursively, add individual indexed files, or drag items here. Source files are never moved or deleted.",\n        );\n'''
if old not in text:
    raise SystemExit('collections settings header block not found')
text = text.replace(old, new, 1)
path.write_text(text, encoding='utf-8')
