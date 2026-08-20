from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")


def replace_one(old: str, new: str, label: str) -> None:
    global text
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label} count={count}")
    text = text.replace(old, new, 1)


# build_visual_descriptors is shared by indexing and on-demand similarity search.
# Only indexing owns an IndexControl, so the helper accepts optional pause control.
replace_one(
    """    '''fn build_visual_descriptors(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    control: &IndexControl,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)""",
    """    '''fn build_visual_descriptors(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    control: Option<&IndexControl>,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
''',
)""",
    "visual signature replacement",
)

replace_one(
    """    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, control, tx)?;
''',
)""",
    """    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, Some(control), tx)?;
''',
)""",
    "rescan visual call replacement",
)

replace_one(
    """    '''    for batch in paths.chunks(batch_size) {
        control.wait_if_paused();
        let committed_before_batch = committed;
''',
)""",
    """    '''    for batch in paths.chunks(batch_size) {
        if let Some(control) = control {
            control.wait_if_paused();
        }
        let committed_before_batch = committed;
''',
)""",
    "visual batch pause replacement",
)

replace_one(
    """    '''                .filter_map(|path| {
                    control.wait_if_paused();
                    let _ = tx.send(WorkerMessage::CurrentFile(
                        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
                    ));
                    let result = decode_image(path).map(|image| {
''',
)""",
    """    '''                .filter_map(|path| {
                    if let Some(control) = control {
                        control.wait_if_paused();
                    }
                    let _ = tx.send(WorkerMessage::CurrentFile(
                        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),
                    ));
                    let result = decode_image(path).map(|image| {
''',
)""",
    "visual decode pause replacement",
)

# v8 changes the rescan replacement to replace_first, leaving the second four-argument
# call (similarity-search backfill) untouched. Patch that remaining call after the
# rescan replacement has executed.
anchor = "# build_visual_descriptors signature/pause.\n"
addition = """replace_once(
    "src/indexer.rs",
    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;
''',
    '''        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, None, tx)?;
''',
)
"""
if addition not in text:
    if text.count(anchor) != 1:
        raise SystemExit(f"visual helper anchor count={text.count(anchor)}")
    text = text.replace(anchor, anchor + addition, 1)

path.write_text(text, encoding="utf-8")
print("made visual descriptor pause control optional")
