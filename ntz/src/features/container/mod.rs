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
    #[error("no container named `{0}`")]
    NotFound(String),

    #[error("a container named `{0}` already exists")]
    NameTaken(String),

    #[error("unsupported language `{0}`")]
    UnsupportedLanguage(String),
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
