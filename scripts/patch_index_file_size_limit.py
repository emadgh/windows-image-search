from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected exactly one match, found {count}: {old[:140]!r}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


indexer = Path("src/indexer.rs")
replace_once(
    indexer,
    "    let mut candidates = HashMap::<PathBuf, PathBuf>::new();\n    let mut removed_targets = Vec::<PathBuf>::new();\n",
    "    let mut candidates = HashMap::<PathBuf, PathBuf>::new();\n    let mut removed_targets = Vec::<PathBuf>::new();\n    let mut oversized_skipped = 0usize;\n",
)
replace_once(
    indexer,
    '''            if changed.is_file() {\n                if is_supported_image(&changed) {\n                    candidates.insert(changed, root.clone());\n                }\n            } else if changed.is_dir() {\n''',
    '''            if changed.is_file() {\n                if is_supported_image(&changed) {\n                    let oversized = std::fs::metadata(&changed)\n                        .map(|meta| !indexing_settings.allows_file_size(meta.len()))\n                        .unwrap_or(false);\n                    if oversized {\n                        oversized_skipped += 1;\n                        // Excluded files must also leave a previously-built index.\n                        removed_targets.push(changed);\n                    } else {\n                        candidates.insert(changed, root.clone());\n                    }\n                }\n            } else if changed.is_dir() {\n''',
)
replace_once(
    indexer,
    '''                        Ok(entry)\n                            if entry.file_type().is_file() && is_supported_image(entry.path()) =>\n                        {\n                            candidates.insert(entry.into_path(), root.clone());\n                        }\n''',
    '''                        Ok(entry)\n                            if entry.file_type().is_file() && is_supported_image(entry.path()) =>\n                        {\n                            let path = entry.into_path();\n                            let oversized = std::fs::metadata(&path)\n                                .map(|meta| !indexing_settings.allows_file_size(meta.len()))\n                                .unwrap_or(false);\n                            if oversized {\n                                oversized_skipped += 1;\n                                removed_targets.push(path);\n                            } else {\n                                candidates.insert(path, root.clone());\n                            }\n                        }\n''',
)
replace_once(
    indexer,
    '''    if !removed_paths.is_empty() {\n        portable::remove_absolute_paths(roots, &removed_paths)?;\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }\n    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n''',
    '''    if !removed_paths.is_empty() {\n        portable::remove_absolute_paths(roots, &removed_paths)?;\n        let _ = tx.send(WorkerMessage::RemovedPaths(removed_paths.clone()));\n    }\n    if oversized_skipped > 0 {\n        let _ = tx.send(WorkerMessage::Status(format!(\n            "Skipped {oversized_skipped} oversized source file{} above the {} MiB indexing limit",\n            if oversized_skipped == 1 { "" } else { "s" },\n            indexing_settings.max_file_size_mib\n        )));\n    }\n    let _ = tx.send(WorkerMessage::RootCounts(db::load_root_counts(db_path)?));\n''',
)
replace_once(
    indexer,
    '''            "Live index synchronized: 0 changed, {} removed",\n            removed_paths.len()\n''',
    '''            "Live index synchronized: 0 changed, {} removed, {oversized_skipped} oversized skipped",\n            removed_paths.len()\n''',
)
replace_once(
    indexer,
    '''        "Live index synchronized: {} changed, {} removed",\n        committed_paths.len(),\n        removed_paths.len()\n''',
    '''        "Live index synchronized: {} changed, {} removed, {oversized_skipped} oversized skipped",\n        committed_paths.len(),\n        removed_paths.len()\n''',
)

replace_once(
    indexer,
    '''    let mut traversal_errors = 0usize;\n    let mut prunable_roots = Vec::<PathBuf>::new();\n''',
    '''    let mut traversal_errors = 0usize;\n    let mut oversized_skipped = 0usize;\n    let mut prunable_roots = Vec::<PathBuf>::new();\n''',
)
replace_once(
    indexer,
    '''                Ok(entry) => {\n                    if entry.file_type().is_file() && is_supported_image(entry.path()) {\n                        candidates.push((root.clone(), entry.into_path()));\n                    }\n                }\n''',
    '''                Ok(entry) => {\n                    if entry.file_type().is_file() && is_supported_image(entry.path()) {\n                        let path = entry.into_path();\n                        let oversized = std::fs::metadata(&path)\n                            .map(|meta| !indexing_settings.allows_file_size(meta.len()))\n                            .unwrap_or(false);\n                        if oversized {\n                            oversized_skipped += 1;\n                        } else {\n                            candidates.push((root.clone(), path));\n                        }\n                    }\n                }\n''',
)
replace_once(
    indexer,
    '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Discovered {discovered_marked}/{total} image paths; checking index state…"\n    )));\n''',
    '''    let _ = tx.send(WorkerMessage::Status(format!(\n        "Discovered {discovered_marked}/{total} eligible image paths; skipped {oversized_skipped} above {} MiB; checking index state…",\n        indexing_settings.max_file_size_mib\n    )));\n''',
)
replace_once(
    indexer,
    '''        "Index ready: {total} image{} (recursive scan, {traversal_errors} traversal error{})",\n        if total == 1 { "" } else { "s" },\n        if traversal_errors == 1 { "" } else { "s" }\n''',
    '''        "Index ready: {total} eligible image{} ({oversized_skipped} oversized skipped, recursive scan, {traversal_errors} traversal error{})",\n        if total == 1 { "" } else { "s" },\n        if traversal_errors == 1 { "" } else { "s" }\n''',
)

ui = Path("src/ui/mod.rs")
replace_once(
    ui,
    '''                    "Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16 / Device CPU.",\n''',
    '''                    "Detected {logical_threads} logical CPU thread{}. Safe defaults: Decode 2 / CLIP up to 4 / Batch 16 / Device CPU / Max file 256 MiB.",\n''',
)
replace_once(
    ui,
    '''                    let batch_changed = ui\n                        .add(\n                            egui::Slider::new(\n                                &mut self.indexing_settings.batch_size,\n                                1..=settings::MAX_BATCH_SIZE,\n                            )\n                            .text("Index / embedding batch size"),\n                        )\n                        .changed();\n\n                    if decode_changed || clip_changed || provider_changed || batch_changed {\n''',
    '''                    let batch_changed = ui\n                        .add(\n                            egui::Slider::new(\n                                &mut self.indexing_settings.batch_size,\n                                1..=settings::MAX_BATCH_SIZE,\n                            )\n                            .text("Index / embedding batch size"),\n                        )\n                        .changed();\n\n                    let max_file_size_changed = ui\n                        .add(\n                            egui::Slider::new(\n                                &mut self.indexing_settings.max_file_size_mib,\n                                1..=settings::MAX_FILE_SIZE_MIB,\n                            )\n                            .logarithmic(true)\n                            .text("Maximum source file size (MiB)"),\n                        )\n                        .changed();\n                    ui.small(format!(\n                        "Files larger than {} MiB are skipped before decode, metadata extraction and CLIP, regardless of extension.",\n                        self.indexing_settings.max_file_size_mib\n                    ));\n\n                    if decode_changed\n                        || clip_changed\n                        || provider_changed\n                        || batch_changed\n                        || max_file_size_changed\n                    {\n''',
)

print("patched extension-agnostic source file size limit")
