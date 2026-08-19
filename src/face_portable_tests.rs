use crate::{db, face_detection, face_store, portable};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "wis-face-portable-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn sample_face() -> face_detection::DetectedFace {
    face_detection::DetectedFace {
        confidence: 0.96,
        bbox: face_detection::FaceBox {
            x: 0.2,
            y: 0.2,
            width: 0.3,
            height: 0.4,
        },
        landmarks: vec![face_detection::FaceLandmark { x: 0.3, y: 0.35 }],
    }
}

#[test]
fn portable_sync_preserves_faces_for_metadata_only_updates_and_invalidates_source_changes() {
    let root = temp_dir("sync");
    std::fs::create_dir_all(root.join("people")).unwrap();
    let source = root.join("people").join("person.jpg");
    let session_db = root
        .parent()
        .unwrap()
        .join(format!("wis-face-session-{}.sqlite3", std::process::id()));

    {
        let conn = db::open(&session_db).unwrap();
        db::add_root(&session_db, &root).unwrap();
        db::upsert_image(
            &conn,
            &source,
            &root,
            "person.jpg",
            "jpg",
            100,
            200,
            800,
            600,
            "",
            "",
            [10, 20, 30],
            7,
            &[1.0, 0.0],
        )
        .unwrap();
        db::set_content_fingerprint(&conn, &source, 77).unwrap();
    }

    portable::attach_root(&session_db, &root).unwrap();
    let relative = Path::new("people").join("person.jpg");
    {
        let mut portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        face_store::ensure_schema(&portable_conn).unwrap();
        face_store::replace_detections(
            &mut portable_conn,
            &relative,
            100,
            200,
            1,
            800,
            600,
            "fake-detector",
            "1",
            &[sample_face()],
        )
        .unwrap();
        assert_eq!(face_store::load_faces(&portable_conn, &relative).unwrap().len(), 1);
    }

    // Embedding-only synchronization must update the portable image row in place,
    // not DELETE+INSERT it, otherwise the face FK cascade would erase valid state.
    {
        let mut session = db::open(&session_db).unwrap();
        db::set_embedding(&session, &source, &[0.6, 0.8]).unwrap();
        portable::sync_paths_from_session(&mut session, std::slice::from_ref(&source)).unwrap();
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(face_store::load_faces(&portable_conn, &relative).unwrap().len(), 1);
    }

    // A real source-state change must invalidate detections through the portable
    // image trigger when the reconciled row is updated.
    {
        let mut session = db::open(&session_db).unwrap();
        db::upsert_image(
            &session,
            &source,
            &root,
            "person.jpg",
            "jpg",
            101,
            201,
            800,
            600,
            "",
            "",
            [10, 20, 30],
            8,
            &[1.0, 0.0],
        )
        .unwrap();
        db::set_content_fingerprint(&session, &source, 78).unwrap();
        portable::sync_paths_from_session(&mut session, std::slice::from_ref(&source)).unwrap();
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert!(face_store::load_faces(&portable_conn, &relative).unwrap().is_empty());
        assert!(face_store::load_detection_state(&portable_conn, &relative)
            .unwrap()
            .is_none());
    }

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(&session_db);
    let _ = std::fs::remove_file(format!("{}-wal", session_db.display()));
    let _ = std::fs::remove_file(format!("{}-shm", session_db.display()));
}
