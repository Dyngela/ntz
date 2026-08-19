use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

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

const COLUMNS: &str =
    "id, name, language, source, version, wasm_path, max_retries, retry_backoff_seconds, created_at, updated_at";

pub fn migrate(db: &Db) -> Result<(), DbError> {
    let conn = db.lock();
    conn.execute_batch(SCHEMA)?;

    // M2/M4: added as later milestones needed them. SQLite has no
    // `ADD COLUMN IF NOT EXISTS`, so tolerate a database that already has
    // them (SQLite allows a NOT NULL column via ALTER TABLE only when it
    // carries a non-NULL default, which both new columns do).
    for stmt in [
        "ALTER TABLE containers ADD COLUMN wasm_path TEXT",
        "ALTER TABLE containers ADD COLUMN max_retries INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE containers ADD COLUMN retry_backoff_seconds INTEGER NOT NULL DEFAULT 0",
    ] {
        match conn.execute(stmt, []) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

pub fn create(db: &Db, container: &Container) -> Result<(), DbError> {
    let result = db.lock().execute(
        "INSERT INTO containers (id, name, language, source, version, wasm_path, max_retries, retry_backoff_seconds, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            container.id,
            container.name,
            container.language.as_str(),
            container.source,
            container.version,
            container.wasm_path,
            container.max_retries,
            container.retry_backoff_seconds,
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
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM containers ORDER BY created_at"))?;

    let rows = stmt.query_map([], row_to_container)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn find(db: &Db, id: &str) -> Result<Option<Container>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM containers WHERE id = ?1"))?;

    stmt.query_row(params![id], row_to_container)
        .optional()
        .map_err(Into::into)
}

/// Persists the result of a build: the artifact path and the version it was
/// built as. Never touches `source` — a build compiles what's already
/// there, it doesn't change it.
pub fn update_build(
    db: &Db,
    id: &str,
    version: i64,
    wasm_path: &str,
    updated_at: DateTime<Utc>,
) -> Result<(), DbError> {
    db.lock().execute(
        "UPDATE containers SET version = ?1, wasm_path = ?2, updated_at = ?3 WHERE id = ?4",
        params![version, wasm_path, updated_at.to_rfc3339(), id],
    )?;
    Ok(())
}

fn row_to_container(row: &rusqlite::Row) -> rusqlite::Result<Container> {
    let language: String = row.get(2)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    Ok(Container {
        id: row.get(0)?,
        name: row.get(1)?,
        language: Language::parse(&language)
            .expect("language column holds only values written by Language::as_str"),
        source: row.get(3)?,
        version: row.get(4)?,
        wasm_path: row.get(5)?,
        max_retries: row.get(6)?,
        retry_backoff_seconds: row.get(7)?,
        created_at: parse_rfc3339(&created_at),
        updated_at: parse_rfc3339(&updated_at),
    })
}

fn parse_rfc3339(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .expect("timestamp columns hold only values written by DateTime::to_rfc3339")
        .with_timezone(&Utc)
}
