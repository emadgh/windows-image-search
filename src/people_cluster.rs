use anyhow::{bail, Result};
use rusqlite::{params, Connection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideAction {
    Ignore,
    ManualMerge,
    ManualSplit,
}

impl OverrideAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::ManualMerge => "manual_merge",
            Self::ManualSplit => "manual_split",
        }
    }
}

pub fn assign_face(
    conn: &Connection,
    person_id: i64,
    face_id: &str,
    confidence: f64,
    assignment_type: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO person_face_members(person_id, face_id, confidence, assignment_type) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(person_id, face_id) DO UPDATE SET confidence=excluded.confidence, assignment_type=excluded.assignment_type",
        params![person_id, face_id, confidence, assignment_type],
    )?;
    Ok(())
}

pub fn remove_face(conn: &Connection, person_id: i64, face_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM person_face_members WHERE person_id=?1 AND face_id=?2",
        params![person_id, face_id],
    )?;
    Ok(())
}

pub fn set_override(conn: &Connection, face_id: &str, action: OverrideAction) -> Result<()> {
    conn.execute(
        "INSERT INTO person_overrides(face_id, action) VALUES(?1, ?2)
         ON CONFLICT(face_id) DO UPDATE SET action=excluded.action",
        params![face_id, action.as_str()],
    )?;
    Ok(())
}

pub fn merge_people(conn: &Connection, source: i64, target: i64) -> Result<()> {
    if source == target {
        bail!("cannot merge a person into itself");
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE person_face_members SET person_id=?1 WHERE person_id=?2",
        params![target, source],
    )?;
    tx.execute("DELETE FROM people WHERE id=?1", params![source])?;
    tx.commit()?;
    Ok(())
}

pub fn split_face(conn: &Connection, person_id: i64, face_id: &str, new_person: &str) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("INSERT INTO people(name) VALUES(?1)", params![new_person])?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "DELETE FROM person_face_members WHERE person_id=?1 AND face_id=?2",
        params![person_id, face_id],
    )?;
    tx.execute(
        "INSERT INTO person_face_members(person_id, face_id, confidence, assignment_type) VALUES(?1, ?2, 1.0, 'manual')",
        params![id, face_id],
    )?;
    tx.commit()?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people_store;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        people_store::ensure_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn merge_moves_members() {
        let conn = db();
        let a = people_store::create_person(&conn, "A").unwrap();
        let b = people_store::create_person(&conn, "B").unwrap();
        assign_face(&conn, a, "face-1", 0.9, "automatic").unwrap();
        merge_people(&conn, a, b).unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM person_face_members WHERE person_id=?1", params![b], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
    }
}
