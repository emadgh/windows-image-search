from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

# The generic spawn signature's replacement text also appears in spawn_rescan_with_mode,
# which makes replace_once's idempotency guard skip spawn_incremental_update. Match the
# full public function header instead.
old_spawn = '''replace_once(
    "src/indexer.rs",
    \'\'\'    embedding_service: EmbeddingService,\n    tx: Sender<WorkerMessage>,\n) {\n\'\'\',
    \'\'\'    embedding_service: EmbeddingService,\n    control: IndexControl,\n    tx: Sender<WorkerMessage>,\n) {\n\'\'\',
)'''
new_spawn = '''replace_once(
    "src/indexer.rs",
    \'\'\'pub fn spawn_incremental_update(\n    db_path: PathBuf,\n    roots: Vec<PathBuf>,\n    changed_paths: Vec<PathBuf>,\n    indexing_settings: IndexingSettings,\n    embedding_service: EmbeddingService,\n    tx: Sender<WorkerMessage>,\n) {\n\'\'\',
    \'\'\'pub fn spawn_incremental_update(\n    db_path: PathBuf,\n    roots: Vec<PathBuf>,\n    changed_paths: Vec<PathBuf>,\n    indexing_settings: IndexingSettings,\n    embedding_service: EmbeddingService,\n    control: IndexControl,\n    tx: Sender<WorkerMessage>,\n) {\n\'\'\',
)'''
if new_spawn not in text:
    if text.count(old_spawn) != 1:
        raise SystemExit(f"spawn patch block count={text.count(old_spawn)}")
    text = text.replace(old_spawn, new_spawn, 1)

# Only the full-rescan descriptor backfill belongs to the pausable indexing pipeline.
# Similarity-search's on-demand descriptor upgrade has no IndexControl.
old_visual = '''replace_all(
    "src/indexer.rs",
    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;\n\'\'\',
    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, control, tx)?;\n\'\'\',
)'''
new_visual = old_visual.replace("replace_all(", "replace_first(", 1)
if new_visual not in text:
    if text.count(old_visual) != 1:
        raise SystemExit(f"visual patch block count={text.count(old_visual)}")
    text = text.replace(old_visual, new_visual, 1)

# FileState gained dimensions/fingerprint. Keep the existing cache regression test
# authoritative for those fields too.
anchor = '''# ---------------------------------------------------------------------------\n# Thumbnail cache validation helper for force-rescan and CLIP inputs\n# ---------------------------------------------------------------------------\n'''
addition = '''replace_once(
    "src/db.rs",
    \'\'\'                Some(&FileState {\n                    size: 111,\n                    modified: 1001,\n                    has_embedding: false,\n                })\n\'\'\',
    \'\'\'                Some(&FileState {\n                    size: 111,\n                    modified: 1001,\n                    width: 32,\n                    height: 32,\n                    content_fingerprint: None,\n                    has_embedding: false,\n                })\n\'\'\',
)
replace_once(
    "src/db.rs",
    \'\'\'                Some(&FileState {\n                    size: 222,\n                    modified: 2002,\n                    has_embedding: true,\n                })\n\'\'\',
    \'\'\'                Some(&FileState {\n                    size: 222,\n                    modified: 2002,\n                    width: 64,\n                    height: 64,\n                    content_fingerprint: None,\n                    has_embedding: true,\n                })\n\'\'\',
)

'''
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"thumbnail section anchor count={text.count(anchor)}")
    text = text.replace(anchor, addition + anchor, 1)

path.write_text(text, encoding="utf-8")
print("fixed alpha4 compile integration contexts")
