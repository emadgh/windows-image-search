from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

helper_anchor = '''def insert_once(path: str, marker: str, addition: str, *, before: bool = True) -> None:\n'''
helper = '''def replace_all(path: str, old: str, new: str) -> None:
    text = read(path)
    if old not in text:
        if new in text:
            return
        raise SystemExit(f"{path}: expected at least one match: {old[:180]!r}")
    write(path, text.replace(old, new))


'''
if helper not in text:
    if helper_anchor not in text:
        raise SystemExit("insert_once anchor missing")
    text = text.replace(helper_anchor, helper + helper_anchor, 1)

old = '''replace_once(
    "src/indexer.rs",
    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;\n\'\'\',
    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, control, tx)?;\n\'\'\',
)'''
new = old.replace("replace_once(", "replace_all(", 1)
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"expected one visual-backfill patch call, found {text.count(old)}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
print("made visual-backfill pause patch apply to all call sites")
