use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub db_path: PathBuf,
    pub max_source_bytes: usize,
    pub max_invoke_bytes: usize,
    pub toolchain_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub scheduler_tick_seconds: u64,
    pub max_concurrent_runs: usize,
    pub run_lease_minutes: i64,
    /// `catch_up: backfill` safety cap — how many missed slots a single
    /// trigger will replay in one go before giving up and jumping to "now".
    /// Without this, a trigger that ran a fine-grained cron and was down for
    /// a long time could try to replay thousands of slots in one tick.
    pub max_backfill_slots: usize,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: env_parsed("NTZ_PORT").unwrap_or(8080),
            db_path: std::env::var("NTZ_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("ntz.db")),
            max_source_bytes: env_parsed("NTZ_MAX_SOURCE_BYTES").unwrap_or(256 * 1024),
            max_invoke_bytes: env_parsed("NTZ_MAX_INVOKE_BYTES").unwrap_or(1024 * 1024),
            toolchain_dir: std::env::var("NTZ_TOOLCHAIN_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| default_toolchain_dir()),
            artifacts_dir: std::env::var("NTZ_ARTIFACTS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("artifacts")),
            scheduler_tick_seconds: env_parsed("NTZ_SCHEDULER_TICK_SECONDS").unwrap_or(30),
            max_concurrent_runs: env_parsed("NTZ_MAX_CONCURRENT_RUNS").unwrap_or(4),
            run_lease_minutes: env_parsed("NTZ_RUN_LEASE_MINUTES").unwrap_or(10),
            max_backfill_slots: env_parsed("NTZ_MAX_BACKFILL_SLOTS").unwrap_or(20),
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
