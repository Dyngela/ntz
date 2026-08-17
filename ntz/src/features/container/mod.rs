/**
 * Container feature.
 * Containers are the building blocks of the application. Each container represents a program that run independently.
 * Program language derive from an enum of supported languages.
 */

pub mod handler;

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

#[allow(dead_code)] // wired up once the store lands
pub struct Container {
    pub name: String,
    pub language: Language,
}
