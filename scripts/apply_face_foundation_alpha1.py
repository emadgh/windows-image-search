from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:140]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once("Cargo.toml", 'version = "0.2.10"', 'version = "0.3.0-alpha.1"')
replace_once(
    "src/main.rs",
    "mod embedding;\nmod fs_watch;\n",
    "mod embedding;\nmod face_detection;\nmod face_store;\nmod fs_watch;\n",
)
