use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::{Run, RunStatus};
use crate::db::{is_unique_violation, Db, DbError};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
    id                 TEXT PRIMARY KEY,
    container_id       TEXT NOT NULL REFERENCES containers(id),
    container_version  INTEGER NOT NULL,
    trigger_id         TEXT REFERENCES triggers(id),
    scheduled_for      TEXT NOT NULL,
    status             TEXT NOT NULL,
    attempt            INTEGER NOT NULL DEFAULT 1,
    started_at         TEXT,
    finished_at        TEXT,
    stdout             TEXT,
    stderr             TEXT,
    error              TEXT,
    leased_until       TEXT,
    retry_at           TEXT,
    UNIQUE(container_id, scheduled_for)
);
";

const COLUMNS: &str = "id, container_id, container_version, trigger_id, scheduled_for, status, \
                        attempt, started_at, finished_at, stdout, stderr, error, leased_until, retry_at";

pub fn migrate(db: &Db) -> Result<(), DbError> {
    db.lock().execute_batch(SCHEMA)?;
    Ok(())
}

/// What happened when the scheduler tried to take ownership of a
/// `(container_id, scheduled_for)` slot for execution.
pub enum Claim {
    /// Nobody had this slot yet — first attempt.
    New(Run),
    /// Re-claimed a `pending` row: either a retry, or one a crash sweep
    /// reset after an abandoned attempt. `attempt` carries over from that
    /// row, it is not reset to 1.
    Resumed(Run),
    /// Someone already owns this slot (still `running` with a live lease,
    /// or it already reached a terminal status) — nothing to do. This is
    /// what makes a slot idempotent across retries of the reconciliation
    /// loop itself.
    AlreadyClaimed,
}

/// The idempotency boundary for scheduled slots: this is the only place a
/// `(container_id, scheduled_for)` pair is ever turned into ownership of a
/// run. A plain `INSERT ... ON CONFLICT` can't express this because
/// resuming needs to *update* an existing `pending` row rather than insert
/// a new one — the same update-in-place rule the retry design settled on.
pub fn claim_slot(
    db: &Db,
    new_run_id: &str,
    container_id: &str,
    container_version: i64,
    trigger_id: &str,
    scheduled_for: DateTime<Utc>,
    lease_minutes: i64,
) -> Result<Claim, DbError> {
    let conn = db.lock();
    let now = Utc::now();
    let leased_until = now + TimeDelta::minutes(lease_minutes);

    let resumed = conn.execute(
        "UPDATE runs SET status = 'running', started_at = ?1, leased_until = ?2
         WHERE container_id = ?3 AND scheduled_for = ?4 AND status = 'pending'",
        params![
            now.to_rfc3339(),
            leased_until.to_rfc3339(),
            container_id,
            scheduled_for.to_rfc3339(),
        ],
    )?;
    if resumed > 0 {
        let run = find_by_slot(&conn, container_id, scheduled_for)?
            .expect("just updated this exact (container_id, scheduled_for) row");
        return Ok(Claim::Resumed(run));
    }

    let result = conn.execute(
        "INSERT INTO runs (id, container_id, container_version, trigger_id, scheduled_for, status, attempt, started_at, leased_until)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', 1, ?6, ?7)",
        params![
            new_run_id,
            container_id,
            container_version,
            trigger_id,
            scheduled_for.to_rfc3339(),
            now.to_rfc3339(),
            leased_until.to_rfc3339(),
        ],
    );

    match result {
        Ok(_) => {
            let run = find_by_slot(&conn, container_id, scheduled_for)?
                .expect("just inserted this exact (container_id, scheduled_for) row");
            Ok(Claim::New(run))
        }
        Err(e) if is_unique_violation(&e) => Ok(Claim::AlreadyClaimed),
        Err(e) => Err(e.into()),
    }
}

/// Reclaims a specific run that's `pending` a retry, by id rather than by
/// slot — its `scheduled_for` never changes on retry, only `attempt` does.
/// `None` means someone else already claimed it first.
pub fn claim_retry(db: &Db, run_id: &str, lease_minutes: i64) -> Result<Option<Run>, DbError> {
    let conn = db.lock();
    let now = Utc::now();
    let leased_until = now + TimeDelta::minutes(lease_minutes);

    let updated = conn.execute(
        "UPDATE runs SET status = 'running', started_at = ?1, leased_until = ?2
         WHERE id = ?3 AND status = 'pending'",
        params![now.to_rfc3339(), leased_until.to_rfc3339(), run_id],
    )?;
    if updated == 0 {
        return Ok(None);
    }
    find_by_id(&conn, run_id).map_err(Into::into)
}

/// Every run currently waiting out its backoff delay, across every trigger —
/// deliberately independent of any trigger's own `next_run_at`, since a
/// retry must not wait for the trigger's *next* cron occurrence.
pub fn find_pending_retries(db: &Db, now: DateTime<Utc>) -> Result<Vec<Run>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM runs
         WHERE status = 'pending' AND attempt > 1 AND retry_at IS NOT NULL AND retry_at <= ?1"
    ))?;

    let rows = stmt.query_map(params![now.to_rfc3339()], row_to_run)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn mark_terminal(
    db: &Db,
    run_id: &str,
    status: RunStatus,
    stdout: &str,
    stderr: &str,
    error: Option<&str>,
    finished_at: DateTime<Utc>,
) -> Result<(), DbError> {
    db.lock().execute(
        "UPDATE runs SET status = ?1, stdout = ?2, stderr = ?3, error = ?4, finished_at = ?5, leased_until = NULL, retry_at = NULL
         WHERE id = ?6",
        params![
            status.as_str(),
            stdout,
            stderr,
            error,
            finished_at.to_rfc3339(),
            run_id,
        ],
    )?;
    Ok(())
}

/// Puts the row back to `pending` with the attempt counter bumped and
/// `retry_at` set to the backoff deadline — `find_pending_retries` is what
/// picks it back up once that deadline passes.
pub fn mark_retry(
    db: &Db,
    run_id: &str,
    next_attempt: i64,
    retry_at: DateTime<Utc>,
) -> Result<(), DbError> {
    db.lock().execute(
        "UPDATE runs SET status = 'pending', attempt = ?1, leased_until = NULL, retry_at = ?2 WHERE id = ?3",
        params![next_attempt, retry_at.to_rfc3339(), run_id],
    )?;
    Ok(())
}

/// A `running` row whose lease expired is presumed crashed mid-execution —
/// not "still going". Resetting it to `pending` (with `retry_at` cleared,
/// so it's eligible immediately, not after a backoff it never actually
/// waited out) lets `claim_slot` resume it on the next tick, same attempt
/// count, as if it were a fresh retry.
pub fn sweep_abandoned(db: &Db, now: DateTime<Utc>) -> Result<usize, DbError> {
    let affected = db.lock().execute(
        "UPDATE runs SET status = 'pending', leased_until = NULL, retry_at = NULL
         WHERE status = 'running' AND leased_until < ?1",
        params![now.to_rfc3339()],
    )?;
    Ok(affected)
}

/// Unconditional insert of an already-finished run — used for direct
/// API/webhook invokes, which are synchronous and already have their
/// outcome by the time anything touches the database. No lease/reservation
/// phase: there's no crash-recovery concern for something that already
/// completed in-process before this call.
pub fn record(db: &Db, run: &Run) -> Result<(), DbError> {
    db.lock().execute(
        "INSERT INTO runs (id, container_id, container_version, trigger_id, scheduled_for, status, attempt, started_at, finished_at, stdout, stderr, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            run.id,
            run.container_id,
            run.container_version,
            run.trigger_id,
            run.scheduled_for.to_rfc3339(),
            run.status.as_str(),
            run.attempt,
            run.started_at.map(|t| t.to_rfc3339()),
            run.finished_at.map(|t| t.to_rfc3339()),
            run.stdout,
            run.stderr,
            run.error,
        ],
    )?;
    Ok(())
}

pub fn list_for_container(db: &Db, container_id: &str) -> Result<Vec<Run>, DbError> {
    let conn = db.lock();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM runs WHERE container_id = ?1 ORDER BY scheduled_for DESC"
    ))?;

    let rows = stmt.query_map(params![container_id], row_to_run)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn find_by_slot(
    conn: &Connection,
    container_id: &str,
    scheduled_for: DateTime<Utc>,
) -> rusqlite::Result<Option<Run>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM runs WHERE container_id = ?1 AND scheduled_for = ?2"
    ))?;
    stmt.query_row(params![container_id, scheduled_for.to_rfc3339()], row_to_run)
        .optional()
}

fn find_by_id(conn: &Connection, run_id: &str) -> rusqlite::Result<Option<Run>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM runs WHERE id = ?1"))?;
    stmt.query_row(params![run_id], row_to_run).optional()
}

fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<Run> {
    let scheduled_for: String = row.get(4)?;
    let status: String = row.get(5)?;
    let started_at: Option<String> = row.get(7)?;
    let finished_at: Option<String> = row.get(8)?;
    let leased_until: Option<String> = row.get(12)?;
    let retry_at: Option<String> = row.get(13)?;

    Ok(Run {
        id: row.get(0)?,
        container_id: row.get(1)?,
        container_version: row.get(2)?,
        trigger_id: row.get(3)?,
        scheduled_for: parse_rfc3339(&scheduled_for),
        status: RunStatus::parse(&status)
            .unwrap_or_else(|| unreachable!("status column holds only values this repo writes, got `{status}`")),
        attempt: row.get(6)?,
        started_at: started_at.map(|s| parse_rfc3339(&s)),
        finished_at: finished_at.map(|s| parse_rfc3339(&s)),
        stdout: row.get(9)?,
        stderr: row.get(10)?,
        error: row.get(11)?,
        leased_until: leased_until.map(|s| parse_rfc3339(&s)),
        retry_at: retry_at.map(|s| parse_rfc3339(&s)),
    })
}

fn parse_rfc3339(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .expect("timestamp columns hold only values written by DateTime::to_rfc3339")
        .with_timezone(&Utc)
}
