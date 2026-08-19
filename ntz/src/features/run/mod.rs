// A run is one concrete execution: which slot it targeted, what happened,
// and whether it's still owned by an in-progress attempt (`leased_until`).
// No `service.rs`/`handler.rs` here on purpose — the only writer is the
// `scheduler` module (reservation, retry, crash sweep), and the only reader
// exposed over HTTP is a plain listing, thin enough to live directly in
// `container::service::list_runs`. If this feature grows real business
// logic of its own later, split it out then.
pub mod repo;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub id: String,
    pub container_id: String,
    pub container_version: i64,
    pub trigger_id: Option<String>,
    /// The cron slot this run targeted — the other half (with
    /// `container_id`) of the idempotency key that stops a slot firing
    /// twice. See `scheduler` module.
    pub scheduled_for: DateTime<Utc>,
    pub status: RunStatus,
    pub attempt: i64,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub error: Option<String>,
    /// Past this instant, a `running` row is presumed abandoned (crashed
    /// mid-execution) and gets swept back to `pending`.
    pub leased_until: Option<DateTime<Utc>>,
    /// Set only when `status: Pending` after a failure (`attempt > 1`) —
    /// when the backoff delay is over and this specific run becomes
    /// eligible to be reclaimed. Independent of the trigger's own cron
    /// schedule: a retry doesn't wait for the *next* scheduled slot.
    pub retry_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Failed,
    /// Not produced yet — arrives with M5's wall-clock timeout. Kept in the
    /// schema/enum now so a later ALTER isn't needed for this one.
    Timeout,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "success" => Some(Self::Success),
            "failed" => Some(Self::Failed),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

impl Serialize for RunStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
