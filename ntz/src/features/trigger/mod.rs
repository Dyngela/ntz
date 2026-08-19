// A trigger is a named way to invoke a container from outside. Two kinds:
// 'webhook' (a stable, container-id-independent URL slug) and 'schedule'
// (a cron expression, read by the scheduler loop). They share one table
// because the scheduler's `next_run_at`/`last_run_at`/`catch_up` columns are
// meaningless for webhooks — NULL for that kind — not because the two kinds
// are conceptually the same.
pub mod handler;
pub mod repo;
pub mod service;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("no webhook trigger at path `{0}`")]
    WebhookNotFound(String),

    #[error("a webhook trigger already exists at path `{0}`")]
    PathTaken(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Trigger {
    pub id: String,
    pub container_id: String,
    pub kind: TriggerKind,
    /// `Some` only for `kind: Webhook`.
    pub path: Option<String>,
    /// `Some` only for `kind: Schedule`.
    pub cron: Option<String>,
    pub enabled: bool,
    /// The scheduler's durability anchor (see `scheduler` module) — `None`
    /// for webhooks, always `Some` for an enabled schedule trigger.
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub catch_up: CatchUp,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Webhook,
    Schedule,
}

impl TriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Schedule => "schedule",
        }
    }
}

impl Serialize for TriggerKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// What happens to missed slots after downtime. See `scheduler` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUp {
    /// Run once for the missed window, then resume on the normal schedule.
    Coalesce,
    /// Run once for every missed slot, oldest first.
    Backfill,
    /// Don't run missed slots at all; just resume on the normal schedule.
    Skip,
}

impl CatchUp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coalesce => "coalesce",
            Self::Backfill => "backfill",
            Self::Skip => "skip",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "coalesce" => Some(Self::Coalesce),
            "backfill" => Some(Self::Backfill),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

impl Serialize for CatchUp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
