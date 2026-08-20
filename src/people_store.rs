use anyhow::Result;
use rusqlite::{params, Connection};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: i64,
    pub name: String,
    pub representative_face_id: Option<String>,
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS people (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL DEFAULT '',
            representative_face_id TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        );

        CREATE TABLE IF NOT EXISTS person_face_members (
            person_id INTEGER NOT NULL,
            face_id TEXT NOT NULL,
            confidence REAL NOT NULL DEFAULT 0,
            assignment_type TEXT NOT NULL DEFAULT 'automatic',
            PRIMARY KEY(person_id, face_id),
            FOREIGN KEY(person_id) REFERENCES people(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS person_overrides (
            face_id TEXT PRIMARY KEY NOT NULL,
            action TEXT NOT NULL,
            metadata TEXT
        );
        "#,
    )?;
    Ok(())
}

pub fn create_person(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute("INSERT INTO people(name) VALUES(?1)", params![name])?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_person(conn: &Connection, id: i64, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE people SET name=?1, updated_at=unixepoch() WHERE id=?2",
        params![name, id],
    )?;
    Ok(())
}

pub fn list_people(conn: &Connection) -> Result<Vec<Person>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, representative_face_id FROM people ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Person {
            id: row.get(0)?,
            name: row.get(1)?,
            representative_face_id: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_schema_and_rename_persist() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        let id = create_person(&conn, "Unknown").unwrap();
        rename_person(&conn, id, "Alice").unwrap();
        assert_eq!(list_people(&conn).unwrap()[0].name, "Alice");
    }
}
