from pathlib import Path

cargo = Path("Cargo.toml")
text = cargo.read_text(encoding="utf-8")
old = 'update-via-github = { git = "https://github.com/emadgh/update-via-github.git" }'
new = 'update-via-github = { git = "https://github.com/emadgh/update-via-github.git", rev = "ec648957782ab800e14ddc6e0a15c694a46f1252" }'
if text.count(old) != 1:
    raise SystemExit("expected one unpinned update-via-github dependency")
cargo.write_text(text.replace(old, new, 1), encoding="utf-8")

indexer = Path("src/indexer.rs")
text = indexer.read_text(encoding="utf-8")
old = 'assert!(err.contains("resized preview is mandatory"));'
new = 'assert!(err.contains("bounded preview is mandatory"));'
if text.count(old) != 1:
    raise SystemExit("expected one stale direct-inspection assertion")
indexer.write_text(text.replace(old, new, 1), encoding="utf-8")
