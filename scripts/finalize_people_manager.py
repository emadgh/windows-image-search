from pathlib import Path

# 1) Preserve an existing cluster anchor when selecting it as representative.
p = Path('src/people_management.rs')
s = p.read_text(encoding='utf-8')
old = '''pub fn set_person_representative(
    conn: &Connection,
    manual_person_id: &str,
    library_id: &str,
    face_id: &str,
) -> Result<()> {
    // Representative selection is an explicit correction for this face, not a cluster anchor.
    people_overrides::assign_face(conn, library_id, face_id, manual_person_id)?;
    people_overrides::set_representative(conn, manual_person_id, library_id, face_id)
}'''
new = '''pub fn set_person_representative(
    conn: &Connection,
    manual_person_id: &str,
    library_id: &str,
    face_id: &str,
) -> Result<()> {
    // Do not downgrade an existing cluster anchor merely because it became the representative.
    let already_assigned_to_person = people_overrides::load_face_overrides(conn)?
        .into_iter()
        .any(|item| {
            item.library_id == library_id
                && item.face_id == face_id
                && item.disposition == people_overrides::FaceOverrideDisposition::Assigned
                && item.manual_person_id.as_deref() == Some(manual_person_id)
        });
    if !already_assigned_to_person {
        people_overrides::assign_face(conn, library_id, face_id, manual_person_id)?;
    }
    people_overrides::set_representative(conn, manual_person_id, library_id, face_id)
}'''
if old not in s:
    raise SystemExit('representative function pattern missing')
s = s.replace(old, new, 1)
p.write_text(s, encoding='utf-8')

# 2) People Manager borrow safety, parent-image counts, and reversible manual exceptions.
p = Path('src/ui/people_manager.rs')
s = p.read_text(encoding='utf-8')
old = '''                self.people_manager_ui.merge_selection.retain(|id| {
                    self.people_manager_ui
                        .catalog
                        .people
                        .iter()
                        .any(|person| &person.person_id == id)
                });'''
new = '''                let valid_person_ids = self
                    .people_manager_ui
                    .catalog
                    .people
                    .iter()
                    .map(|person| person.person_id.clone())
                    .collect::<HashSet<_>>();
                self.people_manager_ui
                    .merge_selection
                    .retain(|id| valid_person_ids.contains(id));'''
if old not in s:
    raise SystemExit('merge selection retain pattern missing')
s = s.replace(old, new, 1)

# Add manual exceptions after the left group list scroll area.
needle = '''                                });
                        },
                    );
                    ui.separator();
                    ui.vertical(|ui| {'''
insert = '''                                });

                            let exceptions = self
                                .people_manager_ui
                                .catalog
                                .members
                                .iter()
                                .filter(|member| {
                                    member.person_id.is_none() && (member.detached || member.ignored)
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            if !exceptions.is_empty() {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.strong(format!("Manual exceptions ({})", exceptions.len()));
                                ui.small("Detached/ignored faces stay here so every correction can be restored.");
                                egui::ScrollArea::vertical()
                                    .id_salt("people-manager-exceptions")
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        for member in exceptions {
                                            ui.horizontal(|ui| {
                                                if let Some(preview) = self.people_preview(
                                                    &member.library_id,
                                                    &member.face_id,
                                                ) {
                                                    if let Some(texture) = self.thumbnail(&preview.image_path) {
                                                        let _ = face_crop_widget(
                                                            ui,
                                                            &texture,
                                                            preview.bbox,
                                                            egui::vec2(42.0, 42.0),
                                                            false,
                                                        );
                                                    }
                                                }
                                                ui.vertical(|ui| {
                                                    ui.small(if member.ignored { "Ignored face" } else { "Detached face" });
                                                    ui.small(&member.face_id);
                                                });
                                                if ui.small_button("Restore").clicked() {
                                                    action = Some(PeopleAction::Restore {
                                                        library_id: member.library_id.clone(),
                                                        face_id: member.face_id.clone(),
                                                    });
                                                }
                                            });
                                        }
                                    });
                            }
                        },
                    );
                    ui.separator();
                    ui.vertical(|ui| {'''
if needle not in s:
    raise SystemExit('left pane insertion point missing')
s = s.replace(needle, insert, 1)

# Compute selected person's members and unique parent-image count before the header.
needle = '''                        let title = person
                            .display_name
                            .clone()
                            .unwrap_or_else(|| "Unnamed person".to_owned());
                        ui.heading(title);
                        ui.small(format!(
                            "{} · {} face{}",
                            match person.source {
                                EffectivePersonSource::Automatic => "Automatic group",
                                EffectivePersonSource::Manual => "Manual identity",
                            },
                            person.member_count,
                            if person.member_count == 1 { "" } else { "s" }
                        ));'''
replacement = '''                        let members = self
                            .people_manager_ui
                            .catalog
                            .members
                            .iter()
                            .filter(|member| {
                                member.person_id.as_deref() == Some(&person.person_id)
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let mut parent_images = HashSet::new();
                        for member in &members {
                            if let Some(preview) =
                                self.people_preview(&member.library_id, &member.face_id)
                            {
                                parent_images.insert(preview.image_path);
                            }
                        }
                        let title = person
                            .display_name
                            .clone()
                            .unwrap_or_else(|| "Unnamed person".to_owned());
                        ui.heading(title);
                        ui.small(format!(
                            "{} · {} face{} · {} image{}",
                            match person.source {
                                EffectivePersonSource::Automatic => "Automatic group",
                                EffectivePersonSource::Manual => "Manual identity",
                            },
                            person.member_count,
                            if person.member_count == 1 { "" } else { "s" },
                            parent_images.len(),
                            if parent_images.len() == 1 { "" } else { "s" }
                        ));'''
if needle not in s:
    raise SystemExit('person header pattern missing')
s = s.replace(needle, replacement, 1)

# Remove the later duplicate member collection.
old = '''                        let members = self
                            .people_manager_ui
                            .catalog
                            .members
                            .iter()
                            .filter(|member| {
                                member.person_id.as_deref() == Some(&person.person_id)
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let selected_face = self.people_manager_ui.selected_face.clone();'''
new = '''                        let selected_face = self.people_manager_ui.selected_face.clone();'''
if old not in s:
    raise SystemExit('duplicate members pattern missing')
s = s.replace(old, new, 1)
p.write_text(s, encoding='utf-8')

# 3) Migration regression: old override table upgrades in place and remains conservative.
p = Path('src/people_overrides.rs')
s = p.read_text(encoding='utf-8')
if 'old_override_schema_migrates_without_promoting_assignments_to_anchors' not in s:
    insert = r'''

    #[test]
    fn old_override_schema_migrates_without_promoting_assignments_to_anchors() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE people_manual_persons (
                manual_person_id TEXT PRIMARY KEY NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                representative_library_id TEXT,
                representative_face_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                updated_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE TABLE people_manual_face_overrides (
                library_id TEXT NOT NULL,
                face_id TEXT NOT NULL,
                disposition TEXT NOT NULL,
                manual_person_id TEXT,
                updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
                PRIMARY KEY(library_id, face_id)
            );
            INSERT INTO people_manual_persons(manual_person_id, display_name)
                VALUES('manual-old', 'Alice');
            INSERT INTO people_manual_face_overrides(
                library_id, face_id, disposition, manual_person_id
            ) VALUES('library-a', 'face-1', 'assigned', 'manual-old');
            "#,
        )
        .unwrap();

        ensure_schema(&conn).unwrap();
        let overrides = load_face_overrides(&conn).unwrap();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].manual_person_id.as_deref(), Some("manual-old"));
        assert!(!overrides[0].propagates_cluster);
    }
'''
    pos = s.rfind('\n}')
    if pos == -1:
        raise SystemExit('people_overrides test module end missing')
    s = s[:pos] + insert + s[pos:]
p.write_text(s, encoding='utf-8')
