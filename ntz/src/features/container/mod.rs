/**
 * Container feature.
 * Containers are the building blocks of the application. Each container represents a program that run independently.
 * Program language derive from an enum of supported languages.
 */

pub mod handler;
pub mod repo;
pub mod service;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Failures this feature can produce. Knows nothing about HTTP — the scheduler
/// and the CLI will consume these too, and neither has a response to write.
/// The status mapping lives in `tools::http`.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("no container `{0}`")]
    NotFound(String),

    #[error("a container named `{0}` already exists")]
    NameTaken(String),

    #[error("unsupported language `{0}`")]
    UnsupportedLanguage(String),

    #[error("build failed: {0}")]
    BuildFailed(String),

    #[error("container `{0}` has never been built")]
    NotBuilt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Go,
}

impl Language {
    pub fn parse(raw: &str) -> Result<Self, ContainerError> {
        match raw {
            "go" => Ok(Self::Go),
            other => Err(ContainerError::UnsupportedLanguage(other.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
        }
    }
}

impl Serialize for Language {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub language: Language,
    pub source: String,
    pub version: i64,
    /// `None` until the first successful build. Set together with `version`
    /// — see `repo::update_build`.
    pub wasm_path: Option<String>,
    /// Scheduler-only settings (see `scheduler` module): how many times a
    /// *scheduled* run retries on failure, and the delay between attempts.
    /// Direct API/webhook invokes never retry — the caller is already
    /// waiting synchronously for a result.
    pub max_retries: i64,
    pub retry_backoff_seconds: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The result of actually running a container's current build once, however
/// it was triggered (direct API call or webhook). A non-zero `exit_code` is
/// the callee reporting its own failure — that's still `success: false`
/// here, not an `Err` at the HTTP boundary; only a host-level failure (no
/// build yet, a trap, ...) is.
#[derive(Debug, Clone, Serialize)]
pub struct InvokeOutcome {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}
