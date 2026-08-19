use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub db_path: PathBuf,
    pub max_source_bytes: usize,
    pub toolchain_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: env_parsed("NTZ_PORT").unwrap_or(8080),
            db_path: std::env::var("NTZ_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("ntz.db")),
            max_source_bytes: env_parsed("NTZ_MAX_SOURCE_BYTES").unwrap_or(256 * 1024),
            toolchain_dir: std::env::var("NTZ_TOOLCHAIN_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| default_toolchain_dir()),
        }
    }
}

/// `%LOCALAPPDATA%\ntz\toolchain` on Windows (a cache dir, not roaming
/// profile data); falls back to a dotdir for other platforms in case this
/// ever runs somewhere else.
fn default_toolchain_dir() -> PathBuf {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local_app_data).join("ntz").join("toolchain");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("ntz").join("toolchain");
    }
    PathBuf::from(".ntz-toolchain")
}

fn env_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    std::env::var(key).ok()?.parse().ok()
}
