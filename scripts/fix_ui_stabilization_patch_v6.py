from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

helper_anchor = '''def insert_once(path: str, marker: str, addition: str, *, before: bool = True) -> None:\n'''
helper = '''def insert_first(path: str, marker: str, addition: str) -> None:
    text = read(path)
    if addition in text:
        return
    if marker not in text:
        raise SystemExit(f"{path}: expected at least one marker: {marker[:180]!r}")
    write(path, text.replace(marker, addition + marker, 1))


'''
if helper not in text:
    if helper_anchor not in text:
        raise SystemExit("insert_once anchor missing")
    text = text.replace(helper_anchor, helper + helper_anchor, 1)

old = '''insert_once(
    "src/indexer.rs",
    "    #[test]\\n",
    \'\'\'    #[test]
    fn index_control_pause_resume_is_cooperative() {'''
new = old.replace("insert_once(", "insert_first(", 1)
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"expected one indexer test insertion call, found {text.count(old)}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
print("disambiguated alpha4 test insertion")
