from pathlib import Path

main = Path("src/main.rs")
text = main.read_text(encoding="utf-8")
old = "mod face_scope;\nmod face_store;"
new = "mod face_scope;\nmod face_similarity;\nmod face_store;"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"main module anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
main.write_text(text, encoding="utf-8")

cargo = Path("Cargo.toml")
text = cargo.read_text(encoding="utf-8")
old = 'version = "0.3.0-alpha.5"'
new = 'version = "0.3.0-alpha.6"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.toml version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
cargo.write_text(text, encoding="utf-8")

lock = Path("Cargo.lock")
text = lock.read_text(encoding="utf-8")
old = 'name = "windows-image-search"\nversion = "0.3.0-alpha.5"'
new = 'name = "windows-image-search"\nversion = "0.3.0-alpha.6"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.lock version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
lock.write_text(text, encoding="utf-8")

print("face similarity alpha6 integration applied")
