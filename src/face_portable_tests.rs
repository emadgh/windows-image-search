use crate::{
    db, face_detection, face_store, people_effective, people_management, people_overrides,
    people_store, portable,
};
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

fn session_db_for(root: &Path, label: &str) -> PathBuf {
    root.parent().unwrap().join(format!(
        "{}-{label}.sqlite3",
        root.file_name().unwrap().to_string_lossy()
    ))
}

fn cleanup(root: &Path, session_db: &Path) {
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_file(session_db);
    let _ = std::fs::remove_file(format!("{}-wal", session_db.display()));
    let _ = std::fs::remove_file(format!("{}-shm", session_db.display()));
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

fn people_state() -> people_store::PeopleClusterState {
    people_store::PeopleClusterState {
        embedding: people_store::PeopleEmbeddingRevision {
            model_id: "sface".to_owned(),
            model_version: "1".to_owned(),
            model_cache_revision: "portable-test".to_owned(),
            dimension: 128,
            alignment_revision: 2,
        },
        algorithm_revision: people_store::ALGORITHM_REVISION,
        similarity_threshold: 0.62,
        min_cluster_size: 2,
    }
}

#[test]
fn portable_sync_preserves_faces_for_metadata_only_updates_and_invalidates_source_changes() {
    let root = temp_dir("sync");
    std::fs::create_dir_all(root.join("people")).unwrap();
    let source = root.join("people").join("person.jpg");
    let session_db = session_db_for(&root, "session");

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
        assert_eq!(
            face_store::load_faces(&portable_conn, &relative)
                .unwrap()
                .len(),
            1
        );
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
        assert_eq!(
            face_store::load_faces(&portable_conn, &relative)
                .unwrap()
                .len(),
            1
        );
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
        assert!(face_store::load_faces(&portable_conn, &relative)
            .unwrap()
            .is_empty());
        assert!(face_store::load_detection_state(&portable_conn, &relative)
            .unwrap()
            .is_none());
    }

    cleanup(&root, &session_db);
}

#[test]
fn automatic_people_snapshot_survives_root_detach_and_rehydrates_on_reattach() {
    let root = temp_dir("people-auto-detach");
    std::fs::create_dir_all(&root).unwrap();
    let session_db = session_db_for(&root, "session");
    let attached = portable::attach_root(&session_db, &root).unwrap();

    let cluster = people_store::PersonCluster {
        person_id: "person-portable".to_owned(),
        representative_library_id: attached.library_id.clone(),
        representative_face_id: "face-1".to_owned(),
        member_count: 1,
    };
    let member = people_store::PersonClusterMember {
        library_id: attached.library_id.clone(),
        face_id: "face-1".to_owned(),
        person_id: Some(cluster.person_id.clone()),
        assignment_similarity: Some(1.0),
        is_outlier: false,
    };

    {
        let mut session = db::open(&session_db).unwrap();
        people_store::replace_automatic_snapshot(
            &mut session,
            &people_state(),
            std::slice::from_ref(&cluster),
            std::slice::from_ref(&member),
        )
        .unwrap();
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(
            people_store::load_members(&portable_conn).unwrap(),
            vec![member.clone()]
        );
    }

    db::remove_root(&session_db, &root).unwrap();
    {
        let session = db::open(&session_db).unwrap();
        assert!(people_store::load_members(&session).unwrap().is_empty());
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(
            people_store::load_members(&portable_conn).unwrap(),
            vec![member.clone()]
        );
    }

    portable::attach_root(&session_db, &root).unwrap();
    {
        let session = db::open(&session_db).unwrap();
        assert_eq!(people_store::load_members(&session).unwrap(), vec![member]);
        assert_eq!(
            people_store::load_clusters(&session).unwrap(),
            vec![cluster]
        );
    }

    cleanup(&root, &session_db);
}

#[test]
fn manual_people_edits_survive_root_detach_and_rehydrate_on_reattach() {
    let root = temp_dir("people-manual-detach");
    std::fs::create_dir_all(&root).unwrap();
    let session_db = session_db_for(&root, "session");
    let attached = portable::attach_root(&session_db, &root).unwrap();

    let manual_id = {
        let session = db::open(&session_db).unwrap();
        people_management::split_face_to_new_person(
            &session,
            &attached.library_id,
            "face-1",
            "Alice",
        )
        .unwrap()
    };
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        let person = people_overrides::load_person(&portable_conn, &manual_id)
            .unwrap()
            .unwrap();
        assert_eq!(person.display_name, "Alice");
        assert_eq!(
            people_overrides::load_face_overrides(&portable_conn)
                .unwrap()
                .len(),
            1
        );
    }

    db::remove_root(&session_db, &root).unwrap();
    {
        let session = db::open(&session_db).unwrap();
        people_management::refresh_manual_cache_from_portable_roots(&session).unwrap();
        assert!(people_overrides::load_people(&session).unwrap().is_empty());
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(
            people_overrides::load_person(&portable_conn, &manual_id)
                .unwrap()
                .unwrap()
                .display_name,
            "Alice"
        );
    }

    portable::attach_root(&session_db, &root).unwrap();
    {
        let session = db::open(&session_db).unwrap();
        people_management::refresh_manual_cache_from_portable_roots(&session).unwrap();
        let person = people_overrides::load_person(&session, &manual_id)
            .unwrap()
            .unwrap();
        assert_eq!(person.display_name, "Alice");
        let overrides = people_overrides::load_face_overrides(&session).unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].library_id, attached.library_id);
    }

    cleanup(&root, &session_db);
}

#[test]
fn shared_root_people_catalog_excludes_faces_outside_current_collection_membership() {
    let root = temp_dir("people-shared-root");
    let keep_folder = root.join("keep");
    let removed_folder = root.join("removed");
    std::fs::create_dir_all(&keep_folder).unwrap();
    std::fs::create_dir_all(&removed_folder).unwrap();
    let session_db = session_db_for(&root, "session");
    let attached = portable::attach_root(&session_db, &root).unwrap();

    let collection = db::create_collection(&session_db, "Scoped people").unwrap();
    db::add_collection_folders(
        &session_db,
        collection.id,
        &[keep_folder.clone(), removed_folder.clone()],
    )
    .unwrap();

    let keep_relative = Path::new("keep").join("keep.jpg");
    let removed_relative = Path::new("removed").join("removed.jpg");
    let (keep_face_id, removed_face_id) = {
        let mut portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        db::upsert_image(
            &portable_conn,
            &keep_relative,
            Path::new("."),
            "keep.jpg",
            "jpg",
            10,
            20,
            100,
            100,
            "",
            "",
            [1, 2, 3],
            1,
            &[1.0, 0.0],
        )
        .unwrap();
        db::upsert_image(
            &portable_conn,
            &removed_relative,
            Path::new("."),
            "removed.jpg",
            "jpg",
            10,
            20,
            100,
            100,
            "",
            "",
            [1, 2, 3],
            2,
            &[1.0, 0.0],
        )
        .unwrap();
        face_store::ensure_schema(&portable_conn).unwrap();
        let keep = face_store::replace_detections(
            &mut portable_conn,
            &keep_relative,
            10,
            20,
            1,
            100,
            100,
            "fake-detector",
            "1",
            &[sample_face()],
        )
        .unwrap();
        let removed = face_store::replace_detections(
            &mut portable_conn,
            &removed_relative,
            10,
            20,
            1,
            100,
            100,
            "fake-detector",
            "1",
            &[sample_face()],
        )
        .unwrap();
        (keep[0].face_id.clone(), removed[0].face_id.clone())
    };

    let cluster = people_store::PersonCluster {
        person_id: "person-scoped".to_owned(),
        representative_library_id: attached.library_id.clone(),
        representative_face_id: removed_face_id.clone(),
        member_count: 2,
    };
    let members = vec![
        people_store::PersonClusterMember {
            library_id: attached.library_id.clone(),
            face_id: keep_face_id.clone(),
            person_id: Some(cluster.person_id.clone()),
            assignment_similarity: Some(0.91),
            is_outlier: false,
        },
        people_store::PersonClusterMember {
            library_id: attached.library_id.clone(),
            face_id: removed_face_id.clone(),
            person_id: Some(cluster.person_id.clone()),
            assignment_similarity: Some(1.0),
            is_outlier: false,
        },
    ];
    {
        let mut session = db::open(&session_db).unwrap();
        people_store::replace_automatic_snapshot(
            &mut session,
            &people_state(),
            std::slice::from_ref(&cluster),
            &members,
        )
        .unwrap();
        let catalog = people_effective::load(&session).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].member_count, 2);
    }

    db::remove_collection_folder(&session_db, collection.id, &removed_folder).unwrap();
    {
        let session = db::open(&session_db).unwrap();
        let catalog = people_effective::load(&session).unwrap();
        assert_eq!(catalog.people.len(), 1);
        assert_eq!(catalog.people[0].member_count, 1);
        assert_eq!(catalog.members.len(), 1);
        assert_eq!(catalog.members[0].face_id, keep_face_id);
        assert!(!catalog
            .members
            .iter()
            .any(|member| member.face_id == removed_face_id));
        assert!(db::load_roots(&session_db)
            .unwrap()
            .iter()
            .any(|attached_root| attached_root == &root));
    }
    {
        let portable_conn = db::open(&portable::index_db_path(&root)).unwrap();
        assert_eq!(people_store::load_members(&portable_conn).unwrap().len(), 2);
    }

    cleanup(&root, &session_db);
}
