use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

/// The one thing every feature's `repo.rs` shares: a handle to the SQLite
/// file. Schema and queries stay with each feature — this only owns the
/// connection, since `rusqlite::Connection` is `Send` but not `Sync`.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("db mutex poisoned")
    }
}

/// `Conflict` is the one case a repo's caller acts on directly (a UNIQUE
/// violation); everything else is opaque infrastructure failure and maps to
/// a 500 at the HTTP boundary.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("a row with this unique key already exists")]
    Conflict,

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}

pub fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}
