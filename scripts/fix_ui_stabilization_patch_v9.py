from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

# build_visual_descriptors is shared by indexing and on-demand similarity search.
# Make pause control optional so only indexing paths block on Pause.
old_sig = '''    \'\'\'\'fn build_visual_descriptors(\n    conn: &mut rusqlite::Connection,\n    paths: &[PathBuf],\n    indexing_settings: IndexingSettings,\n    control: &IndexControl,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',\n)'''
new_sig = '''    \'\'\'\'fn build_visual_descriptors(\n    conn: &mut rusqlite::Connection,\n    paths: &[PathBuf],\n    indexing_settings: IndexingSettings,\n    control: Option<&IndexControl>,\n    tx: &Sender<WorkerMessage>,\n) -> Result<()> {\n\'\'\',\n)'''
if new_sig not in text:
    if text.count(old_sig) != 1:
        raise SystemExit(f"visual signature replacement count={text.count(old_sig)}")
    text = text.replace(old_sig, new_sig, 1)

old_rescan_call = '''    \'\'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, control, tx)?;\n\'\'\',\n)'''
new_rescan_call = '''    \'\'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, Some(control), tx)?;\n\'\'\',\n)'''
if new_rescan_call not in text:
    if text.count(old_rescan_call) != 1:
        raise SystemExit(f"rescan visual call replacement count={text.count(old_rescan_call)}")
    text = text.replace(old_rescan_call, new_rescan_call, 1)

old_batch = '''    \'\'\'\'    for batch in paths.chunks(batch_size) {\n        control.wait_if_paused();\n        let committed_before_batch = committed;\n\'\'\',\n)'''
new_batch = '''    \'\'\'\'    for batch in paths.chunks(batch_size) {\n        if let Some(control) = control {\n            control.wait_if_paused();\n        }\n        let committed_before_batch = committed;\n\'\'\',\n)'''
if new_batch not in text:
    if text.count(old_batch) != 1:
        raise SystemExit(f"visual batch pause replacement count={text.count(old_batch)}")
    text = text.replace(old_batch, new_batch, 1)

old_decode = '''    \'\'\'\'                .filter_map(|path| {\n                    control.wait_if_paused();\n                    let _ = tx.send(WorkerMessage::CurrentFile(\n                        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),\n                    ));\n                    let result = decode_image(path).map(|image| {\n\'\'\',\n)'''
new_decode = '''    \'\'\'\'                .filter_map(|path| {\n                    if let Some(control) = control {\n                        control.wait_if_paused();\n                    }\n                    let _ = tx.send(WorkerMessage::CurrentFile(\n                        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),\n                    ));\n                    let result = decode_image(path).map(|image| {\n\'\'\',\n)'''
if new_decode not in text:
    if text.count(old_decode) != 1:
        raise SystemExit(f"visual decode pause replacement count={text.count(old_decode)}")
    text = text.replace(old_decode, new_decode, 1)

# After v8, only the similarity-search descriptor-upgrade call remains in four-arg form.
anchor = '''# build_visual_descriptors signature/pause.\n'''
addition = '''replace_once(\n    "src/indexer.rs",\n    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;\n\'\'\',\n    \'\'\'        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, None, tx)?;\n\'\'\',\n)\n'''
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"visual helper anchor count={text.count(anchor)}")
    text = text.replace(anchor, anchor + addition, 1)

path.write_text(text, encoding="utf-8")
print("made visual descriptor pause control optional")
