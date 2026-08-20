from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")
old = '''replace_once(
    "src/ui/mod.rs",
    \'\'\'                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.thumb_pool.set_roots(self.roots.clone());\n\'\'\',
    \'\'\'                self.roots = db::load_roots(&self.db_path).unwrap_or_default();\n                self.root_counts = db::load_root_counts(&self.db_path).unwrap_or_default();\n                self.thumb_pool.set_roots(self.roots.clone());\n\'\'\',
)'''
new = old.replace("replace_once(", "replace_first(", 1)
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"expected one root-count patch call, found {text.count(old)}")
    text = text.replace(old, new, 1)
path.write_text(text, encoding="utf-8")
print("disambiguated root count refresh patch")
