from pathlib import Path

path = Path("scripts/apply_ui_stabilization_alpha4.py")
text = path.read_text(encoding="utf-8")

blocks = [
'''replace_once(
    "src/indexer.rs",
    \'\'\'                .filter_map(|item| {\n                    let result = inspect_image(&item.path, &item.root).map(\n\'\'\',
    \'\'\'                .filter_map(|item| {\n                    control.wait_if_paused();\n                    let _ = tx.send(WorkerMessage::CurrentFile(\n                        item.path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned(),\n                    ));\n                    let result = inspect_pending_image(item).map(\n\'\'\',
)''',
'''replace_once(
    "src/indexer.rs",
    \'\'\'            indexing_settings,\n            embedding_service,\n            tx,\n        ) {\n\'\'\',
    \'\'\'            indexing_settings,\n            embedding_service,\n            roots,\n            false,\n            control,\n            tx,\n        ) {\n\'\'\',
)''',
'''replace_once(
    "src/ui/mod.rs",
    \'\'\'            self.indexing_settings,\n            self.embedding_service.clone(),\n            self.tx.clone(),\n        );\n\'\'\',
    \'\'\'            self.indexing_settings,\n            self.embedding_service.clone(),\n            control,\n            self.tx.clone(),\n        );\n\'\'\',
)''',
]

for old in blocks:
    new = old.replace("replace_once(", "replace_first(", 1)
    if new in text:
        continue
    if text.count(old) != 1:
        raise SystemExit(f"expected one patch-call block, found {text.count(old)}: {old[:160]!r}")
    text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
print("disambiguated remaining live-index alpha4 patch calls")
