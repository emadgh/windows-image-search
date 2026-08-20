from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")
old = '''replace_once(
    "src/indexer.rs",
    \'\'\'    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',
    \'\'\'    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    control: &IndexControl,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',
)'''
new = '''replace_once(
    "src/indexer.rs",
    \'\'\'fn incremental_update(\n    db_path: &Path,\n    roots: &[PathBuf],\n    changed_paths: &[PathBuf],\n    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',
    \'\'\'fn incremental_update(\n    db_path: &Path,\n    roots: &[PathBuf],\n    changed_paths: &[PathBuf],\n    indexing_settings: IndexingSettings,\n    embedding_service: &EmbeddingService,\n    control: &IndexControl,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',
)'''
if new not in text:
    if text.count(old) != 1:
        raise SystemExit(f"expected one ambiguous incremental signature block, found {text.count(old)}")
    text = text.replace(old, new, 1)
path.write_text(text, encoding="utf-8")
print("narrowed incremental_update patch context")
