use crate::{people_effective, people_overrides};
use anyhow::{bail, Context, Result};
use rusqlite::Connection;

pub fn materialize_effective_person(
    conn: &Connection,
    effective_person_id: &str,
    display_name: Option<&str>,
) -> Result<String> {
    let catalog = people_effective::load(conn)?;
    let person = catalog
        .people
        .iter()
        .find(|person| person.person_id == effective_person_id)
        .with_context(|| format!("effective Person does not exist: {effective_person_id}"))?;

    if person.source == people_effective::EffectivePersonSource::Manual {
        if let Some(name) = display_name {
            people_overrides::rename_person(conn, &person.person_id, name)?;
        }
        return Ok(person.person_id.clone());
    }

    let mut members = catalog
        .members
        .iter()
        .filter(|member| {
            member.person_id.as_deref() == Some(effective_person_id)
                && member.source == Some(people_effective::EffectivePersonSource::Automatic)
        })
        .map(|member| (member.library_id.clone(), member.face_id.clone()))
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    if members.is_empty() {
        bail!("automatic Person has no effective members: {effective_person_id}");
    }

    let representative = person
        .representative_library_id
        .clone()
        .zip(person.representative_face_id.clone())
        .filter(|candidate| members.contains(candidate))
        .unwrap_or_else(|| members[0].clone());
    let manual = people_overrides::create_person(
        conn,
        display_name.unwrap_or_default(),
        &representative.0,
        &representative.1,
    )?;
    for (library_id, face_id) in &members {
        people_overrides::anchor_face(conn, library_id, face_id, &manual.manual_person_id)?;
    }
    people_overrides::set_representative(
        conn,
        &manual.manual_person_id,
        &representative.0,
        &representative.1,
    )?;
    Ok(manual.manual_person_id)
}

pub fn rename_effective_person(
    conn: &Connection,
    effective_person_id: &str,
    display_name: &str,
) -> Result<String> {
    materialize_effective_person(conn, effective_person_id, Some(display_name))
}

pub fn merge_effective_people(
    conn: &mut Connection,
    effective_person_ids: &[String],
    display_name: Option<&str>,
) -> Result<String> {
    let mut ids = effective_person_ids
        .iter()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.len() < 2 {
        bail!("People merge requires at least two distinct groups");
    }

    let mut manual_ids = Vec::with_capacity(ids.len());
    for id in &ids {
        manual_ids.push(materialize_effective_person(conn, id, None)?);
    }
    manual_ids.sort();
    manual_ids.dedup();
    let keep = manual_ids
        .first()
        .cloned()
        .context("People merge produced no manual Person")?;
    let merge = manual_ids.iter().skip(1).cloned().collect::<Vec<_>>();
    people_overrides::merge_people(conn, &keep, &merge)?;
    if let Some(name) = display_name {
        people_overrides::rename_person(conn, &keep, name)?;
    }
    Ok(keep)
}

pub fn move_face_to_person(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    manual_person_id: &str,
) -> Result<()> {
    people_overrides::assign_face(conn, library_id, face_id, manual_person_id)
}

pub fn split_face_to_new_person(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
    display_name: &str,
) -> Result<String> {
    let person = people_overrides::create_person(conn, display_name, library_id, face_id)?;
    people_overrides::assign_face(conn, library_id, face_id, &person.manual_person_id)?;
    people_overrides::set_representative(conn, &person.manual_person_id, library_id, face_id)?;
    Ok(person.manual_person_id)
}

pub fn detach_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::detach_face(conn, library_id, face_id)
}

pub fn ignore_face(conn: &Connection, library_id: &str, face_id: &str) -> Result<()> {
    people_overrides::ignore_face(conn, library_id, face_id)
}

pub fn restore_automatic_face(
    conn: &Connection,
    library_id: &str,
    face_id: &str,
) -> Result<bool> {
    people_overrides::clear_face_override(conn, library_id, face_id)
}

pub fn set_person_representative(
    conn: &Connection,
    manual_person_id: &str,
    library_id: &str,
    face_id: &str,
) -> Result<()> {
    // Representative selection is an explicit correction for this face, not a cluster anchor.
    people_overrides::assign_face(conn, library_id, face_id, manual_person_id)?;
    people_overrides::set_representative(conn, manual_person_id, library_id, face_id)
}

pub fn delete_manual_person(conn: &Connection, manual_person_id: &str) -> Result<bool> {
    people_overrides::delete_person(conn, manual_person_id)
}
