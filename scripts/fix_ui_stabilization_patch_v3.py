from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

helper_anchor = '''def insert_once(path: str, marker: str, addition: str, *, before: bool = True) -> None:\n'''
helper = '''def replace_first(path: str, old: str, new: str) -> None:
    text = read(path)
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"{path}: expected at least one match: {old[:180]!r}")
    write(path, text.replace(old, new, 1))


'''
if helper not in text:
    if helper_anchor not in text:
        raise SystemExit("insert_once anchor missing")
    text = text.replace(helper_anchor, helper + helper_anchor, 1)

old = '''replace_once(
    "src/indexer.rs",
    \'\'\'    for batch in pending.chunks(batch_size) {\n        let prepared: Vec<PreparedImage> = pool.install(|| {\n\'\'\',
    \'\'\'    for batch in pending.chunks(batch_size) {\n        control.wait_if_paused();\n        let prepared: Vec<PreparedImage> = pool.install(|| {\n\'\'\',
)'''
new = old.replace("replace_once(", "replace_first(", 1)
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"expected one incremental batch patch call, found {text.count(old)}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
print("disambiguated incremental batch patch")
