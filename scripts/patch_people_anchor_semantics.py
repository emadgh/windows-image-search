from pathlib import Path

p = Path('src/people_overrides.rs')
s = p.read_text(encoding='utf-8')

s = s.replace(
'''pub struct FaceOverride {\n    pub library_id: String,\n    pub face_id: String,\n    pub disposition: FaceOverrideDisposition,\n    pub manual_person_id: Option<String>,\n}''',
'''pub struct FaceOverride {\n    pub library_id: String,\n    pub face_id: String,\n    pub disposition: FaceOverrideDisposition,\n    pub manual_person_id: Option<String>,\n    pub propagates_cluster: bool,\n}''')

s = s.replace(
'''            manual_person_id TEXT,\n            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),''',
'''            manual_person_id TEXT,\n            propagates_cluster INTEGER NOT NULL DEFAULT 0 CHECK(propagates_cluster IN (0, 1)),\n            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),''')

needle = '''    )?;\n    Ok(())\n}\n\npub fn create_person'''
replacement = '''    )?;\n\n    let has_propagates_cluster = {\n        let mut stmt = conn.prepare("PRAGMA table_info(people_manual_face_overrides)")?;\n        let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;\n        let mut found = false;\n        for column in columns {\n            if column? == "propagates_cluster" {\n                found = true;\n                break;\n            }\n        }\n        found\n    };\n    if !has_propagates_cluster {\n        conn.execute(\n            "ALTER TABLE people_manual_face_overrides ADD COLUMN propagates_cluster INTEGER NOT NULL DEFAULT 0 CHECK(propagates_cluster IN (0, 1))",\n            [],\n        )?;\n    }\n    Ok(())\n}\n\npub fn create_person'''
if needle not in s:
    raise SystemExit('ensure_schema insertion point missing')
s = s.replace(needle, replacement, 1)

start = s.index('pub fn assign_face(')
end = s.index('\npub fn detach_face', start)
s = s[:start] + '''pub fn assign_face(\n    conn: &Connection,\n    library_id: &str,\n    face_id: &str,\n    manual_person_id: &str,\n) -> Result<()> {\n    assign_face_with_propagation(conn, library_id, face_id, manual_person_id, false)\n}\n\npub fn anchor_face(\n    conn: &Connection,\n    library_id: &str,\n    face_id: &str,\n    manual_person_id: &str,\n) -> Result<()> {\n    assign_face_with_propagation(conn, library_id, face_id, manual_person_id, true)\n}\n\nfn assign_face_with_propagation(\n    conn: &Connection,\n    library_id: &str,\n    face_id: &str,\n    manual_person_id: &str,\n    propagates_cluster: bool,\n) -> Result<()> {\n    ensure_schema(conn)?;\n    validate_face_key(library_id, face_id)?;\n    validate_person_id(manual_person_id)?;\n    if load_person(conn, manual_person_id)?.is_none() {\n        bail!("manual Person does not exist: {manual_person_id}");\n    }\n    upsert_face_override(\n        conn,\n        library_id,\n        face_id,\n        FaceOverrideDisposition::Assigned,\n        Some(manual_person_id),\n        propagates_cluster,\n    )\n}\n''' + s[end:]

s = s.replace(
'''        FaceOverrideDisposition::Detached,\n        None,\n    )''',
'''        FaceOverrideDisposition::Detached,\n        None,\n        false,\n    )''')
s = s.replace(
'''        FaceOverrideDisposition::Ignored,\n        None,\n    )''',
'''        FaceOverrideDisposition::Ignored,\n        None,\n        false,\n    )''')

s = s.replace(
'''        SELECT library_id, face_id, disposition, manual_person_id\n        FROM people_manual_face_overrides''',
'''        SELECT library_id, face_id, disposition, manual_person_id, propagates_cluster\n        FROM people_manual_face_overrides''')
s = s.replace(
'''            row.get::<_, Option<String>>(3)?,\n        ))''',
'''            row.get::<_, Option<String>>(3)?,\n            row.get::<_, i64>(4)? != 0,\n        ))''', 1)
s = s.replace(
'''        let (library_id, face_id, disposition, manual_person_id) = row?;\n        output.push(FaceOverride {\n            library_id,\n            face_id,\n            disposition: FaceOverrideDisposition::parse(&disposition)?,\n            manual_person_id,\n        });''',
'''        let (library_id, face_id, disposition, manual_person_id, propagates_cluster) = row?;\n        output.push(FaceOverride {\n            library_id,\n            face_id,\n            disposition: FaceOverrideDisposition::parse(&disposition)?,\n            manual_person_id,\n            propagates_cluster,\n        });''')

s = s.replace(
'''    disposition: FaceOverrideDisposition,\n    manual_person_id: Option<&str>,\n) -> Result<()> {''',
'''    disposition: FaceOverrideDisposition,\n    manual_person_id: Option<&str>,\n    propagates_cluster: bool,\n) -> Result<()> {''')
s = s.replace(
'''            library_id, face_id, disposition, manual_person_id, updated_at\n        ) VALUES(?1, ?2, ?3, ?4, unixepoch())\n        ON CONFLICT(library_id, face_id) DO UPDATE SET\n            disposition = excluded.disposition,\n            manual_person_id = excluded.manual_person_id,\n            updated_at = unixepoch()''',
'''            library_id, face_id, disposition, manual_person_id, propagates_cluster, updated_at\n        ) VALUES(?1, ?2, ?3, ?4, ?5, unixepoch())\n        ON CONFLICT(library_id, face_id) DO UPDATE SET\n            disposition = excluded.disposition,\n            manual_person_id = excluded.manual_person_id,\n            propagates_cluster = excluded.propagates_cluster,\n            updated_at = unixepoch()''')
s = s.replace(
'''        params![library_id, face_id, disposition.as_str(), manual_person_id],''',
'''        params![\n            library_id,\n            face_id,\n            disposition.as_str(),\n            manual_person_id,\n            if propagates_cluster { 1i64 } else { 0i64 }\n        ],''')

p.write_text(s, encoding='utf-8')

p = Path('src/people_effective.rs')
s = p.read_text(encoding='utf-8')
s = s.replace(
'''                    (item.disposition == people_overrides::FaceOverrideDisposition::Assigned)\n                        .then(|| item.manual_person_id.clone())''',
'''                    (item.disposition == people_overrides::FaceOverrideDisposition::Assigned\n                        && item.propagates_cluster)\n                        .then(|| item.manual_person_id.clone())''')

# Existing tests that intentionally verify cluster inheritance/conflict should use anchors.
s = s.replace(
'''people_overrides::assign_face(&conn, "library-a", "face-1", &manual.manual_person_id)\n            .unwrap();''',
'''people_overrides::anchor_face(&conn, "library-a", "face-1", &manual.manual_person_id)\n            .unwrap();''', 1)

# Add explicit split regression before the final test module brace.
insert = r'''

    #[test]
    fn explicit_assignment_does_not_claim_unoverridden_cluster_members() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        write_auto(
            &mut conn,
            vec![auto_cluster("person-auto", "face-1", 3)],
            vec![
                auto_member("face-1", Some("person-auto")),
                auto_member("face-2", Some("person-auto")),
                auto_member("face-3", Some("person-auto")),
            ],
        );
        let manual =
            people_overrides::create_person(&conn, "Bob", "library-a", "face-3").unwrap();
        people_overrides::assign_face(&conn, "library-a", "face-3", &manual.manual_person_id)
            .unwrap();

        let catalog = load(&conn).unwrap();
        let manual_members = catalog
            .members
            .iter()
            .filter(|member| member.person_id.as_deref() == Some(&manual.manual_person_id))
            .count();
        let auto_members = catalog
            .members
            .iter()
            .filter(|member| member.person_id.as_deref() == Some("person-auto"))
            .count();
        assert_eq!(manual_members, 1);
        assert_eq!(auto_members, 2);
    }
'''
pos = s.rfind('\n}')
if pos == -1:
    raise SystemExit('people_effective test module end missing')
s = s[:pos] + insert + s[pos:]
p.write_text(s, encoding='utf-8')
