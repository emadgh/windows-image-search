from pathlib import Path

path = Path('src/people_clustering.rs')
text = path.read_text(encoding='utf-8')
marker = '''    fn test_root(base: &Path, name: &str, library_id: &str) -> PathBuf {\n'''
if marker not in text:
    raise SystemExit('test_root marker missing')
helper = '''    fn test_dir(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-image-search-people-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

'''
text = text.replace(marker, helper + marker, 1)

replacements = {
'''        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");''':
'''        let dir = test_dir("attach");
        let session = dir.join("session.sqlite3");
        let root = test_root(&dir, "root", "library-a");''',
'''        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");''': None,
}
# Occurrences are three tests; replace them sequentially with distinct labels.
old = '''        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("session.sqlite3");
        let root = test_root(dir.path(), "root", "library-a");'''
for label in ['attach', 'promote', 'threshold']:
    if old not in text:
        raise SystemExit(f'tempfile block missing for {label}')
    new = f'''        let dir = test_dir("{label}");
        let session = dir.join("session.sqlite3");
        let root = test_root(&dir, "root", "library-a");'''
    text = text.replace(old, new, 1)

# Add cleanup at the end of each persisted test. Keep it after all SQLite connections from helper calls are dropped.
needle1 = '''            Some(original.as_str())
        );
    }

    #[test]
    fn persisted_incremental_update_promotes_old_outlier_with_new_matching_face()'''
text = text.replace(
    needle1,
    '''            Some(original.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persisted_incremental_update_promotes_old_outlier_with_new_matching_face()''',
    1,
)
needle2 = '''        assert_eq!(carol1.person_id, carol2.person_id);
        assert_eq!(persisted_clusters(&session).len(), 2);
    }

    #[test]
    fn changed_threshold_forces_full_rebuild_instead_of_mixing_snapshots()'''
text = text.replace(
    needle2,
    '''        assert_eq!(carol1.person_id, carol2.person_id);
        assert_eq!(persisted_clusters(&session).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_threshold_forces_full_rebuild_instead_of_mixing_snapshots()''',
    1,
)
needle3 = '''        assert!((state.similarity_threshold - 0.95).abs() < 1e-6);
    }

    #[test]
    fn obvious_same_people_cluster_and_singletons_remain_outliers()'''
text = text.replace(
    needle3,
    '''        assert!((state.similarity_threshold - 0.95).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn obvious_same_people_cluster_and_singletons_remain_outliers()''',
    1,
)

path.write_text(text, encoding='utf-8')
