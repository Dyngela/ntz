use chrono::Utc;
use croner::Cron;

use super::{repo, CatchUp, Trigger, TriggerError, TriggerKind};
use crate::db::DbError;
use crate::features::container;
use crate::tools::http::AppError;
use crate::AppState;

pub async fn create_webhook(
    state: &AppState,
    container_id: String,
    path: String,
) -> Result<Trigger, AppError> {
    if path.trim().is_empty() {
        return Err(AppError::Validation("path must not be empty".to_owned()));
    }
    ensure_container_exists(state, &container_id).await?;

    let trigger = Trigger {
        id: uuid::Uuid::now_v7().to_string(),
        container_id,
        kind: TriggerKind::Webhook,
        path: Some(path),
        cron: None,
        enabled: true,
        next_run_at: None,
        last_run_at: None,
        catch_up: CatchUp::Coalesce, // irrelevant for webhooks; column is NOT NULL
        created_at: Utc::now(),
    };

    let db = state.db.clone();
    let to_persist = trigger.clone();
    let result = tokio::task::spawn_blocking(move || repo::create(&db, &to_persist))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    match result {
        Ok(()) => Ok(trigger),
        Err(DbError::Conflict) => {
            Err(TriggerError::PathTaken(trigger.path.unwrap_or_default()).into())
        }
        Err(e) => Err(e.into()),
    }
}

/// `cron_expr` is standard 5-field cron (`minute hour day month weekday`,
/// no seconds) — validated here so a typo fails at creation, not silently
/// at the next scheduler tick.
pub async fn create_schedule(
    state: &AppState,
    container_id: String,
    cron_expr: String,
    catch_up: CatchUp,
) -> Result<Trigger, AppError> {
    let cron: Cron = cron_expr
        .parse()
        .map_err(|e: croner::errors::CronError| {
            AppError::Validation(format!("invalid cron expression: {e}"))
        })?;
    ensure_container_exists(state, &container_id).await?;

    let now = Utc::now();
    let next_run_at = cron
        .find_next_occurrence(&now, false)
        .map_err(|e| AppError::Internal(e.to_string().into()))?;

    let trigger = Trigger {
        id: uuid::Uuid::now_v7().to_string(),
        container_id,
        kind: TriggerKind::Schedule,
        path: None,
        cron: Some(cron_expr),
        enabled: true,
        next_run_at: Some(next_run_at),
        last_run_at: None,
        catch_up,
        created_at: now,
    };

    let db = state.db.clone();
    let to_persist = trigger.clone();
    tokio::task::spawn_blocking(move || repo::create(&db, &to_persist))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??;

    Ok(trigger)
}

/// Resolves the incoming path to a container and delegates to the exact
/// same `invoke` an API caller would go through — one code path behind both
/// trigger kinds, per the doc's "single chokepoint" principle.
pub async fn handle_webhook(
    state: &AppState,
    path: String,
    body: Vec<u8>,
) -> Result<container::InvokeOutcome, AppError> {
    let db = state.db.clone();
    let lookup_path = path.clone();
    let trigger = tokio::task::spawn_blocking(move || {
        repo::find_enabled_webhook_by_path(&db, &lookup_path)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string().into()))??
    .ok_or(TriggerError::WebhookNotFound(path))?;

    container::service::invoke(state, trigger.container_id, body, Some(trigger.id)).await
}

async fn ensure_container_exists(state: &AppState, container_id: &str) -> Result<(), AppError> {
    let db = state.db.clone();
    let lookup_id = container_id.to_owned();
    let exists = tokio::task::spawn_blocking(move || container::repo::find(&db, &lookup_id))
        .await
        .map_err(|e| AppError::Internal(e.to_string().into()))??
        .is_some();

    if exists {
        Ok(())
    } else {
        Err(container::ContainerError::NotFound(container_id.to_owned()).into())
    }
}
