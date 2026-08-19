use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use super::{CatchUp, Trigger, TriggerKind};
use crate::db::{is_unique_violation, Db, DbError};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS triggers (
    id           TEXT PRIMARY KEY,
    container_id TEXT NOT NULL REFERENCES containers(id),
    kind         TEXT NOT NULL,
    path         TEXT UNIQUE,
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL
);
";

const COLUMNS: &str =
    "id, container_id, kind, path, cron, enabled, next_run_at, last_run_at, catch_up, created_at";

pub fn migrate(db: &Db) -> Result<(), DbError> {
    let conn = db.lock();
    conn.execute_batch(SCHEMA)?;

    // M4: added once schedule triggers existed. SQLite allows a NOT NULL
    // column via ALTER TABLE only when it has a non-NULL default, which is
    // exactly the case for `catch_up`. No `IF NOT EXISTS`, so tolerate a
    // database that already has these.
    for stmt in [
        "ALTER TABLE triggers ADD COLUMN cron TEXT",
        "ALTER TABLE triggers ADD COLUMN next_run_at TEXT",
        "ALTER TABLE triggers ADD COLUMN last_run_at TEXT",
        "ALTER TABLE triggers ADD COLUMN catch_up TEXT NOT NULL DEFAULT 'coalesce'",
    ] {
        match conn.execute(stmt, []) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

pub fn create(db: &Db, trigger: &Trigger) -> Result<(), DbError> {
    let result = db.lock().execute(
        "INSERT INTO triggers (id, container_id, kind, path, cron, enabled, next_run_at, last_run_at, catch_up, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            trigger.id,
            trigger.container_id,
            trigger.kind.as_str(),
            trigger.path,
            trigger.cron,
            trigger.enabled,
            trigger.next_run_at.map(|t| t.to_rfc3339()),
            trigger.last_run_at.map(|t| t.to_rfc3339()),
            trigger.catch_up.as_str(),
            trigger.created_at.to_rfc3339(),
        ],
    );

    match result {
        Ok(_) => Ok(()),
        Err(e) if is_unique_violation(&e) => Err(DbError::Conflict),
        Err(e) => Err(e.into()),
    }
}

pub fn find_enabled_webhook_by_path(db: &Db, path: &str) -> Result<Option<Trigger>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM triggers WHERE kind = 'webhook' AND path = ?1 AND enabled = 1"
    ))?;

    stmt.query_row(params![path], row_to_trigger)
        .optional()
        .map_err(Into::into)
}

/// The scheduler's due-work query: every enabled schedule trigger whose
/// anchor has arrived. `now` is passed in rather than read here so a single
/// tick reasons about one consistent instant throughout.
pub fn find_due_schedules(db: &Db, now: DateTime<Utc>) -> Result<Vec<Trigger>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM triggers
         WHERE kind = 'schedule' AND enabled = 1 AND next_run_at <= ?1"
    ))?;

    let rows = stmt.query_map(params![now.to_rfc3339()], row_to_trigger)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Defensive: a schedule trigger whose stored cron expression no longer
/// parses (shouldn't happen — it's validated at creation — but the parser
/// could change) would otherwise show up as "due" on every single tick
/// forever. Disabling it stops the spam; it stays visible for the same
/// investigation a stuck cron job would get anywhere else.
pub fn disable(db: &Db, trigger_id: &str) -> Result<(), DbError> {
    db.lock().execute(
        "UPDATE triggers SET enabled = 0 WHERE id = ?1",
        params![trigger_id],
    )?;
    Ok(())
}

pub fn update_after_run(
    db: &Db,
    trigger_id: &str,
    last_run_at: DateTime<Utc>,
    next_run_at: DateTime<Utc>,
) -> Result<(), DbError> {
    db.lock().execute(
        "UPDATE triggers SET last_run_at = ?1, next_run_at = ?2 WHERE id = ?3",
        params![last_run_at.to_rfc3339(), next_run_at.to_rfc3339(), trigger_id],
    )?;
    Ok(())
}

fn row_to_trigger(row: &rusqlite::Row) -> rusqlite::Result<Trigger> {
    let kind: String = row.get(2)?;
    let next_run_at: Option<String> = row.get(6)?;
    let last_run_at: Option<String> = row.get(7)?;
    let catch_up: String = row.get(8)?;
    let created_at: String = row.get(9)?;

    Ok(Trigger {
        id: row.get(0)?,
        container_id: row.get(1)?,
        kind: match kind.as_str() {
            "webhook" => TriggerKind::Webhook,
            "schedule" => TriggerKind::Schedule,
            other => unreachable!("kind column holds only values this repo writes, got `{other}`"),
        },
        path: row.get(3)?,
        cron: row.get(4)?,
        enabled: row.get(5)?,
        next_run_at: next_run_at.map(|s| parse_rfc3339(&s)),
        last_run_at: last_run_at.map(|s| parse_rfc3339(&s)),
        catch_up: CatchUp::parse(&catch_up)
            .unwrap_or_else(|| unreachable!("catch_up column holds only values this repo writes, got `{catch_up}`")),
        created_at: parse_rfc3339(&created_at),
    })
}

fn parse_rfc3339(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .expect("timestamp columns hold only values written by DateTime::to_rfc3339")
        .with_timezone(&Utc)
}
