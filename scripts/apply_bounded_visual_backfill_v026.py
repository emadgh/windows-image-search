from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if new in text:
        return
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/indexer.rs",
    '''    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading visual index: {} image{} need texture/color descriptors…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, indexing_settings.decode_workers, tx)?;
    }''',
    '''    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading visual index: {} image{} need texture/color descriptors…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;
    }''',
)

replace_once(
    "src/indexer.rs",
    '''    let indexing_settings = indexing_settings.sanitized();
    let conn = db::open(db_path)?;

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading texture/color index: {} image{}…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&conn, &missing_visual, indexing_settings.decode_workers, tx)?;
    }''',
    '''    let indexing_settings = indexing_settings.sanitized();
    let mut conn = db::open(db_path)?;

    let missing_visual = db::paths_missing_visual_descriptor(&conn)?;
    if !missing_visual.is_empty() {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Upgrading texture/color index: {} image{}…",
            missing_visual.len(),
            if missing_visual.len() == 1 { "" } else { "s" }
        )));
        build_visual_descriptors(&mut conn, &missing_visual, indexing_settings, tx)?;
    }''',
)

replace_once(
    "src/indexer.rs",
    '''fn build_visual_descriptors(
    conn: &rusqlite::Connection,
    paths: &[PathBuf],
    workers: usize,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let total = paths.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.max(1))
        .thread_name(|index| format!("visual-index-{index}"))
        .build()
        .context("creating visual descriptor worker pool")?;
    let done = AtomicUsize::new(0);

    let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>)> = pool.install(|| {
        paths
            .par_iter()
            .filter_map(|path| {
                let result = decode_image(path).map(|image| visual_descriptor(&image));
                let current = done.fetch_add(1, Ordering::Relaxed) + 1;
                if current % 25 == 0 || current == total {
                    let _ = tx.send(WorkerMessage::Status(format!(
                        "Building texture/color index: {current}/{total}"
                    )));
                }
                match result {
                    Ok((_, visual_hash, color_histogram, material_texture)) => {
                        Some((path.clone(), visual_hash, color_histogram, material_texture))
                    }
                    Err(err) => {
                        let _ = tx.send(WorkerMessage::Error(format!(
                            "Cannot build visual descriptor for {}: {err:#}",
                            path.display()
                        )));
                        None
                    }
                }
            })
            .collect()
    });

    for (path, visual_hash, color_histogram, material_texture) in descriptors {
        db::set_visual_descriptor(conn, &path, visual_hash, &color_histogram)?;
        db::set_material_texture(conn, &path, &material_texture)?;
    }
    Ok(())
}''',
    '''fn build_visual_descriptors(
    conn: &mut rusqlite::Connection,
    paths: &[PathBuf],
    indexing_settings: IndexingSettings,
    tx: &Sender<WorkerMessage>,
) -> Result<()> {
    let indexing_settings = indexing_settings.sanitized();
    let total = paths.len();
    if total == 0 {
        return Ok(());
    }

    let workers = indexing_settings.decode_workers;
    let batch_size = indexing_settings.batch_size;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("visual-index-{index}"))
        .build()
        .context("creating visual descriptor worker pool")?;
    let decoded = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let mut committed = 0usize;

    let _ = tx.send(WorkerMessage::Status(format!(
        "Visual descriptor backfill: {total} image{} with {workers} decode worker{}; committing every {batch_size} images…",
        if total == 1 { "" } else { "s" },
        if workers == 1 { "" } else { "s" },
    )));

    for batch in paths.chunks(batch_size) {
        let committed_before_batch = committed;
        // Hold only one bounded descriptor batch in memory. Each successfully
        // decoded batch is committed before the next batch is decoded.
        let descriptors: Vec<(PathBuf, u64, Vec<f32>, Vec<f32>)> = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|path| {
                    let result = decode_image(path).map(|image| visual_descriptor(&image));
                    let current = decoded.fetch_add(1, Ordering::Relaxed) + 1;
                    if current % 16 == 0 || current == total {
                        let _ = tx.send(WorkerMessage::Status(format!(
                            "Visual descriptor backfill: decoded {current}/{total}; committed {committed_before_batch}/{total}"
                        )));
                    }
                    match result {
                        Ok((_, visual_hash, color_histogram, material_texture)) => {
                            Some((path.clone(), visual_hash, color_histogram, material_texture))
                        }
                        Err(err) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            let _ = tx.send(WorkerMessage::Error(format!(
                                "Cannot build visual descriptor for {}: {err:#}",
                                path.display()
                            )));
                            None
                        }
                    }
                })
                .collect()
        });

        if descriptors.is_empty() {
            continue;
        }

        {
            let transaction = conn.transaction()?;
            for (path, visual_hash, color_histogram, material_texture) in &descriptors {
                db::set_visual_descriptor(
                    &transaction,
                    path,
                    *visual_hash,
                    color_histogram,
                )?;
                db::set_material_texture(&transaction, path, material_texture)?;
            }
            transaction.commit()?;
        }

        committed += descriptors.len();
        let _ = tx.send(WorkerMessage::Status(format!(
            "Visual descriptor backfill: committed {committed}/{total} safely stored"
        )));
    }

    let failed = failed.load(Ordering::Relaxed);
    if failed > 0 {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Visual descriptor backfill finished: {committed}/{total} committed; {failed} decode failure{} remain eligible for retry",
            if failed == 1 { "" } else { "s" }
        )));
    } else {
        let _ = tx.send(WorkerMessage::Status(format!(
            "Visual descriptor backfill finished: {committed}/{total} committed"
        )));
    }
    Ok(())
}''',
)

replace_once(
    "src/indexer.rs",
    '''    #[test]
    fn committed_batch_survives_later_rollback() {''',
    '''    #[test]
    fn committed_visual_descriptor_batch_resumes_after_later_rollback() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!(
            "windows-image-search-visual-backfill-durability-{}-{nonce}.sqlite3",
            std::process::id()
        ));
        let root = PathBuf::from("C:/indexed");
        let first = root.join("first.jpg");
        let second = root.join("second.jpg");

        {
            let mut conn = db::open(&db_path).unwrap();
            for (path, name) in [(&first, "first.jpg"), (&second, "second.jpg")] {
                db::upsert_image(
                    &conn,
                    path,
                    &root,
                    name,
                    "jpg",
                    123,
                    456,
                    64,
                    64,
                    "",
                    "",
                    [120, 90, 60],
                    0x55AA_55AA_55AA_55AA,
                    &[1.0, 0.0, 0.0, 0.0],
                )
                .unwrap();
            }

            {
                let transaction = conn.transaction().unwrap();
                db::set_visual_descriptor(
                    &transaction,
                    &first,
                    0x1111_2222_3333_4444,
                    &[0.7, 0.3],
                )
                .unwrap();
                db::set_material_texture(&transaction, &first, &[0.1, 0.2, 0.3]).unwrap();
                transaction.commit().unwrap();
            }
            {
                let transaction = conn.transaction().unwrap();
                db::set_visual_descriptor(
                    &transaction,
                    &second,
                    0xAAAA_BBBB_CCCC_DDDD,
                    &[0.4, 0.6],
                )
                .unwrap();
                db::set_material_texture(&transaction, &second, &[0.3, 0.2, 0.1]).unwrap();
                // Simulate interruption before this descriptor batch commits.
            }
        }

        let conn = db::open(&db_path).unwrap();
        let missing = db::paths_missing_visual_descriptor(&conn).unwrap();
        assert!(!missing.contains(&first));
        assert!(missing.contains(&second));
        drop(conn);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn committed_batch_survives_later_rollback() {''',
)
