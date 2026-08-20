from pathlib import Path

main = Path("src/main.rs")
text = main.read_text(encoding="utf-8")
old = "mod face_detection;\nmod face_pipeline;"
new = "mod face_detection;\nmod face_embedding;\nmod face_embedding_pipeline;\nmod face_embedding_store;\nmod face_pipeline;"
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"main module anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
main.write_text(text, encoding="utf-8")

cargo = Path("Cargo.toml")
text = cargo.read_text(encoding="utf-8")
old = 'version = "0.3.0-alpha.4"'
new = 'version = "0.3.0-alpha.5"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.toml version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
cargo.write_text(text, encoding="utf-8")

lock = Path("Cargo.lock")
text = lock.read_text(encoding="utf-8")
old = 'name = "windows-image-search"\nversion = "0.3.0-alpha.4"'
new = 'name = "windows-image-search"\nversion = "0.3.0-alpha.5"'
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"Cargo.lock package version anchor count={text.count(old)}")
    text = text.replace(old, new, 1)
lock.write_text(text, encoding="utf-8")

print("face embedding alpha5 integration applied")
