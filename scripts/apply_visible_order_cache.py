from pathlib import Path

path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")
old = "#[derive(Clone, Copy, PartialEq, Eq)]\npub(super) enum SortMode {\n"
new = "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub(super) enum SortMode {\n"
if old not in text:
    raise SystemExit("SortMode derive target not found")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
