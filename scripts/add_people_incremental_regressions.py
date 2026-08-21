from pathlib import Path

path = Path('src/people_clustering.rs')
text = path.read_text(encoding='utf-8')
needle = '''mod tests {
    use super::*;

    fn revision()'''
if needle not in text:
    raise SystemExit('People tests module header not found')
replacement = '''mod tests {
    use super::*;
    use crate::{face_embedding_store, face_store};
    use rusqlite::params;

    fn revision()'''
text = text.replace(needle, replacement, 1)

insert_before = '''    #[test]
    fn obvious_same_people_cluster_and_singletons_remain_outliers() {'''
if insert_before not in text:
    raise SystemExit('first People test marker not found')
helpers_and_tests = r'''    fn embedding_blob(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    fn test_root(base: &Path, name: &str, library_id: &str) -> PathBuf {
        let root = base.join(name);
        std::fs::create_dir_all(portable::index_dir(&root)).unwrap();
        let conn = crate::db::open(&portable::index_db_path(&root)).unwrap();
        face_store::ensure_schema(&conn).unwrap();
        face_embedding_store::ensure_schema(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS portable_meta(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO portable_meta(key, value) VALUES('library_id', ?1)",
            params![library_id],
        )
        .unwrap();
        root
    }

    fn insert_test_face(root: &Path, face_id: &str, values: Vec<f32>) {
        let revision = revision();
        assert_eq!(values.len(), revision.dimension);
        let conn = crate::db::open(&portable::index_db_path(root)).unwrap();
        face_store::ensure_schema(&conn).unwrap();
        face_embedding_store::ensure_schema(&conn).unwrap();
        let relative = format!("{face_id}.jpg");
        let root_text = root.to_string_lossy().to_string();
        let detector_id = "test-detector";
        let detector_version = "1";
        let detector_cache_revision = "test-detector-cache";
        let source_size = 100i64;
        let source_modified = 10i64;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO images(
                path, root, file_name, extension, size, modified, width, height
            ) VALUES(?1, ?2, ?3, 'jpg', ?4, ?5, 256, 256)
            "#,
            params![relative, root_text, format!("{face_id}.jpg"), source_size, source_modified],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT OR REPLACE INTO face_detection_state(
                image_path, detector_id, detector_version, detector_cache_revision,
                schema_version, source_size, source_modified,
                exif_orientation, oriented_width, oriented_height, face_count
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 256, 256, 1)
            "#,
            params![
                relative,
                detector_id,
                detector_version,
                detector_cache_revision,
                crate::face_detection::SCHEMA_VERSION,
                source_size,
                source_modified,
            ],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT OR REPLACE INTO faces(
                face_id, image_path, face_ordinal,
                detector_id, detector_version, detector_cache_revision, schema_version,
                confidence, bbox_x, bbox_y, bbox_width, bbox_height,
                landmarks, landmark_count, source_size, source_modified
            ) VALUES(?1, ?2, 0, ?3, ?4, ?5, ?6, 0.99, 0.1, 0.1, 0.5, 0.5, NULL, 0, ?7, ?8)
            "#,
            params![
                face_id,
                relative,
                detector_id,
                detector_version,
                detector_cache_revision,
                crate::face_detection::SCHEMA_VERSION,
                source_size,
                source_modified,
            ],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT OR REPLACE INTO face_embeddings(
                face_id, model_id, model_version, model_cache_revision,
                schema_version, alignment_revision, dimension, normalized, embedding,
                detector_id, detector_version, detector_cache_revision,
                detection_schema_version, source_size, source_modified
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                face_id,
                revision.model_id,
                revision.model_version,
                revision.model_cache_revision,
                face_embedding::SCHEMA_VERSION,
                revision.alignment_revision,
                revision.dimension as i64,
                embedding_blob(&values),
                detector_id,
                detector_version,
                detector_cache_revision,
                crate::face_detection::SCHEMA_VERSION,
                source_size,
                source_modified,
            ],
        )
        .unwrap();
    }

    fn persisted_members(session: &Path) -> Vec<people_store::PersonClusterMember> {
        let conn = crate::db::open(session).unwrap();
        people_store::load_members(&conn).unwrap()
    }

    fn persisted_clusters(session: &Path) -> Vec<people_store::PersonCluster> {
        let conn = crate::db::open(session).unwrap();
        people_store::load_clusters(&conn).unwrap()
    }

    #[test]
    fn persisted_incremental_update_attaches_new_face_to_existing_person() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");
        insert_test_face(&root, "alice-1", unit(1.0, 0.0, 0.0));
        insert_test_face(&root, "alice-2", unit(0.99, 0.02, 0.0));
        let options = PeopleClusteringOptions {
            similarity_threshold: 0.80,
            min_cluster_size: 2,
        };
        run(&session, std::slice::from_ref(&root), &revision(), options).unwrap();
        let original = persisted_clusters(&session)[0].person_id.clone();

        insert_test_face(&root, "alice-3", unit(0.98, -0.02, 0.0));
        let summary = run_incremental(
            &session,
            std::slice::from_ref(&root),
            &revision(),
            options,
        )
        .unwrap();
        let clusters = persisted_clusters(&session);
        let members = persisted_members(&session);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].person_id, original);
        assert_eq!(clusters[0].member_count, 3);
        assert_eq!(summary.faces_clustered, 3);
        assert_eq!(
            members
                .iter()
                .find(|member| member.face_id == "alice-3")
                .and_then(|member| member.person_id.as_deref()),
            Some(original.as_str())
        );
    }

    #[test]
    fn persisted_incremental_update_promotes_old_outlier_with_new_matching_face() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");
        insert_test_face(&root, "alice-1", unit(1.0, 0.0, 0.0));
        insert_test_face(&root, "alice-2", unit(0.99, 0.02, 0.0));
        insert_test_face(&root, "carol-1", unit(0.0, 0.0, 1.0));
        let options = PeopleClusteringOptions {
            similarity_threshold: 0.80,
            min_cluster_size: 2,
        };
        run(&session, std::slice::from_ref(&root), &revision(), options).unwrap();
        assert!(persisted_members(&session)
            .iter()
            .find(|member| member.face_id == "carol-1")
            .unwrap()
            .is_outlier);

        insert_test_face(&root, "carol-2", unit(0.01, 0.0, 0.999));
        run_incremental(
            &session,
            std::slice::from_ref(&root),
            &revision(),
            options,
        )
        .unwrap();
        let members = persisted_members(&session);
        let carol1 = members.iter().find(|member| member.face_id == "carol-1").unwrap();
        let carol2 = members.iter().find(|member| member.face_id == "carol-2").unwrap();
        assert!(!carol1.is_outlier && !carol2.is_outlier);
        assert_eq!(carol1.person_id, carol2.person_id);
        assert_eq!(persisted_clusters(&session).len(), 2);
    }

    #[test]
    fn changed_threshold_forces_full_rebuild_instead_of_mixing_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");
        insert_test_face(&root, "p1", unit(1.0, 0.0, 0.0));
        insert_test_face(&root, "p2", unit(0.90, 0.435, 0.0));
        run(
            &session,
            std::slice::from_ref(&root),
            &revision(),
            PeopleClusteringOptions {
                similarity_threshold: 0.80,
                min_cluster_size: 2,
            },
        )
        .unwrap();
        assert_eq!(persisted_clusters(&session).len(), 1);

        run_incremental(
            &session,
            std::slice::from_ref(&root),
            &revision(),
            PeopleClusteringOptions {
                similarity_threshold: 0.95,
                min_cluster_size: 2,
            },
        )
        .unwrap();
        assert!(persisted_clusters(&session).is_empty());
        assert_eq!(
            persisted_members(&session)
                .iter()
                .filter(|member| member.is_outlier)
                .count(),
            2
        );
        let conn = crate::db::open(&session).unwrap();
        let state = people_store::load_state(&conn).unwrap().unwrap();
        assert!((state.similarity_threshold - 0.95).abs() < 1e-6);
    }

'''
text = text.replace(insert_before, helpers_and_tests + insert_before, 1)
path.write_text(text, encoding='utf-8')
