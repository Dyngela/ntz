use chrono::{DateTime, Utc};
use rusqlite::params;

use super::{Container, Language};
use crate::db::{is_unique_violation, Db, DbError};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS containers (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    language   TEXT NOT NULL,
    source     TEXT NOT NULL,
    version    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

pub fn migrate(db: &Db) -> Result<(), DbError> {
    db.lock().execute_batch(SCHEMA)?;
    Ok(())
}

pub fn create(db: &Db, container: &Container) -> Result<(), DbError> {
    let result = db.lock().execute(
        "INSERT INTO containers (id, name, language, source, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            container.id,
            container.name,
            container.language.as_str(),
            container.source,
            container.version,
            container.created_at.to_rfc3339(),
            container.updated_at.to_rfc3339(),
        ],
    );

    match result {
        Ok(_) => Ok(()),
        Err(e) if is_unique_violation(&e) => Err(DbError::Conflict),
        Err(e) => Err(e.into()),
    }
}

pub fn list(db: &Db) -> Result<Vec<Container>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, language, source, version, created_at, updated_at
         FROM containers ORDER BY created_at",
    )?;

    let rows = stmt.query_map([], |row| {
        let language: String = row.get(2)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;

        Ok(Container {
            id: row.get(0)?,
            name: row.get(1)?,
            language: Language::parse(&language)
                .expect("language column holds only values written by Language::as_str"),
            source: row.get(3)?,
            version: row.get(4)?,
            created_at: parse_rfc3339(&created_at),
            updated_at: parse_rfc3339(&updated_at),
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn parse_rfc3339(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .expect("timestamp columns hold only values written by DateTime::to_rfc3339")
        .with_timezone(&Utc)
}
