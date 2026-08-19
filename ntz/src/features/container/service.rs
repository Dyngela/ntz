use std::path::Path;

use chrono::Utc;

use super::{repo, Container, ContainerError, InvokeOutcome, Language};
use crate::db::DbError;
use crate::toolchain::ToolchainError;
use crate::tools::http::AppError;
use crate::AppState;

/// Compile errors come from the user's own source — worth showing, but
/// capped so a pathological error dump can't blow up the response or the DB.
const MAX_BUILD_ERROR_CHARS: usize = 8 * 1024;

pub async fn create(
    state: &AppState,
    name: String,
    language: String,
    source: String,
    max_retries: i64,
    retry_backoff_seconds: i64,
) -> Result<Container, AppError> {
    let language = Language::parse(&language)?;

    if name.trim().is_empty() {
        return Err(AppError::Validation("name must not be empty".to_owned()));
    }
    if source.trim().is_empty() {
        return Err(AppError::Validation("source must not be empty".to_owned()));
    }
    if source.len() > state.config.max_source_bytes {
        return Err(AppError::Validation(format!(
            "source exceeds the {} byte limit",
            state.config.max_source_bytes
        )));
    }
    if max_retries < 0 {
        return Err(AppError::Validation("max_retries must be >= 0".to_owned()));
    }
    if retry_backoff_seconds < 0 {
        return Err(AppError::Validation(
            "retry_backoff_seconds must be >= 0".to_owned(),
        ));
    }

    let now = Utc::now();
    let container = Container {
        id: uuid::Uuid::now_v7().to_string(),
        name,
        language,
        source,
        version: 1,
        wasm_path: None,
        max_retries,
        retry_backoff_seconds,
        created_at: now,
        updated_at: now,
    };

    let db = state.db.clone();
    let to_persist = container.clone();
    let result = tokio::task::spawn_blocking(move || repo::create(&db, &to_persist))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    match result {
        Ok(()) => Ok(container),
        Err(DbError::Conflict) => Err(ContainerError::NameTaken(container.name).into()),
        Err(e) => Err(e.into()),
    }
}

pub async fn find_all(state: &AppState) -> Result<Vec<Container>, AppError> {
    let db = state.db.clone();
    let containers = tokio::task::spawn_blocking(move || repo::list(&db))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??;

    Ok(containers)
}

pub async fn list_runs(state: &AppState, id: String) -> Result<Vec<crate::features::run::Run>, AppError> {
    let db = state.db.clone();
    let runs = tokio::task::spawn_blocking(move || crate::features::run::repo::list_for_container(&db, &id))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??;

    Ok(runs)
}

/// Compiles the container's current `source` to a fresh `.wasm` artifact.
/// Never overwrites a previous artifact: every successful build gets its own
/// version and its own file, so an old version stays reachable.
pub async fn build(state: &AppState, id: String) -> Result<Container, AppError> {
    let db = state.db.clone();
    let lookup_id = id.clone();
    let container = tokio::task::spawn_blocking(move || repo::find(&db, &lookup_id))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??
        .ok_or(ContainerError::NotFound(id))?;

    // First build keeps the version the row already has; every rebuild after
    // that bumps it, so the filename — and the old artifact — never collide.
    let version = match container.wasm_path {
        Some(_) => container.version + 1,
        None => container.version,
    };
    let wasm_path = state
        .config
        .artifacts_dir
        .join(format!("{}-v{version}.wasm", container.id));

    std::fs::create_dir_all(&state.config.artifacts_dir)
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    state
        .toolchain
        .build_go_wasm(&container.source, &wasm_path)
        .await
        .map_err(build_error)?;

    let now = Utc::now();
    let wasm_path = wasm_path.to_string_lossy().into_owned();

    let db = state.db.clone();
    let update_id = container.id.clone();
    let update_path = wasm_path.clone();
    tokio::task::spawn_blocking(move || {
        repo::update_build(&db, &update_id, version, &update_path, now)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string().into()))??;

    Ok(Container {
        version,
        wasm_path: Some(wasm_path),
        updated_at: now,
        ..container
    })
}

/// Runs the container's current build once with `input` as stdin — nothing
/// else. Whatever the callee does, including exiting non-zero on purpose, is
/// a normal `InvokeOutcome`, not an error; only "there's nothing built to
/// run" or a genuine host-level failure reach the `Err` side.
///
/// Records no history. Used by `invoke` below (which records a completed
/// run unconditionally) and by the scheduler (which manages its own run
/// lifecycle via `run::repo::claim_slot`/`claim_retry`/`mark_terminal` — a
/// scheduled run needs a lease *before* execution starts, not just a row
/// after, so it can't go through the same wrapper).
pub async fn run_once(
    state: &AppState,
    id: &str,
    input: &[u8],
) -> Result<(Container, InvokeOutcome), AppError> {
    if input.len() > state.config.max_invoke_bytes {
        return Err(AppError::Validation(format!(
            "invoke payload exceeds the {} byte limit",
            state.config.max_invoke_bytes
        )));
    }

    let db = state.db.clone();
    let lookup_id = id.to_owned();
    let container = tokio::task::spawn_blocking(move || repo::find(&db, &lookup_id))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??
        .ok_or_else(|| ContainerError::NotFound(id.to_owned()))?;

    let wasm_path = container
        .wasm_path
        .clone()
        .ok_or_else(|| ContainerError::NotBuilt(id.to_owned()))?;

    let host = state.wasmhost.clone();
    let input = input.to_vec();
    let outcome = tokio::task::spawn_blocking(move || host.run(Path::new(&wasm_path), &input))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))?
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    Ok((
        container,
        InvokeOutcome {
            success: outcome.exit_code == 0,
            exit_code: outcome.exit_code,
            stdout: String::from_utf8_lossy(&outcome.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&outcome.stderr).into_owned(),
        },
    ))
}

/// The API/webhook entry point: runs once, then unconditionally records a
/// completed `runs` row (`trigger_id: None` for a direct API call, `Some`
/// for a webhook). If nothing actually executed (not found, not built, host
/// failure) no row is recorded — there's nothing to historize.
pub async fn invoke(
    state: &AppState,
    id: String,
    input: Vec<u8>,
    trigger_id: Option<String>,
) -> Result<InvokeOutcome, AppError> {
    let (container, outcome) = run_once(state, &id, &input).await?;

    let now = Utc::now();
    let run = crate::features::run::Run {
        id: uuid::Uuid::now_v7().to_string(),
        container_id: container.id,
        container_version: container.version,
        trigger_id,
        scheduled_for: now,
        status: if outcome.success {
            crate::features::run::RunStatus::Success
        } else {
            crate::features::run::RunStatus::Failed
        },
        attempt: 1,
        started_at: Some(now),
        finished_at: Some(now),
        stdout: Some(outcome.stdout.clone()),
        stderr: Some(outcome.stderr.clone()),
        error: None,
        leased_until: None,
        retry_at: None,
    };

    let db = state.db.clone();
    let recorded = tokio::task::spawn_blocking(move || crate::features::run::repo::record(&db, &run))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    // Recording history is best-effort from the caller's perspective: the
    // invocation itself already succeeded and produced a real result, so a
    // logging failure shouldn't turn a good response into a 500.
    if let Err(e) = recorded {
        tracing::error!(error = %e, "failed to record run history for invoke");
    }

    Ok(outcome)
}

fn build_error(err: ToolchainError) -> AppError {
    match err {
        ToolchainError::BuildFailed(_, stderr) => {
            let truncated: String = stderr.chars().take(MAX_BUILD_ERROR_CHARS).collect();
            ContainerError::BuildFailed(truncated).into()
        }
        other => AppError::Internal(other.to_string().into()),
    }
}
