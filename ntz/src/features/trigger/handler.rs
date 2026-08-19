use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Router;
use serde::Deserialize;

use super::{service, CatchUp, Trigger};
use crate::features::container::InvokeOutcome;
use crate::tools::http::{AppError, AppJson, Resp, Success};
use crate::AppState;

/// Trigger management — mounted at `/triggers`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhook", axum::routing::post(create_webhook))
        .route("/schedule", axum::routing::post(create_schedule))
}

/// The public-facing receiving end — mounted at `/hooks`, deliberately
/// separate from `/triggers` since this one is meant to be reachable by
/// whatever external service the webhook path was handed to.
pub fn webhook_router() -> Router<AppState> {
    Router::new().route("/{path}", axum::routing::post(invoke_webhook))
}

#[derive(Deserialize)]
pub struct CreateWebhook {
    container_id: String,
    path: String,
}

async fn create_webhook(
    State(state): State<AppState>,
    AppJson(payload): AppJson<CreateWebhook>,
) -> Resp<Trigger> {
    tracing::info!(container_id = %payload.container_id, path = %payload.path, "create webhook trigger");
    let trigger = service::create_webhook(&state, payload.container_id, payload.path).await?;
    Ok(Success::created(trigger))
}

#[derive(Deserialize)]
pub struct CreateSchedule {
    container_id: String,
    cron: String,
    #[serde(default = "default_catch_up")]
    catch_up: String,
}

fn default_catch_up() -> String {
    "coalesce".to_owned()
}

async fn create_schedule(
    State(state): State<AppState>,
    AppJson(payload): AppJson<CreateSchedule>,
) -> Resp<Trigger> {
    tracing::info!(container_id = %payload.container_id, cron = %payload.cron, "create schedule trigger");
    let catch_up = CatchUp::parse(&payload.catch_up).ok_or_else(|| {
        AppError::Validation(format!(
            "catch_up must be one of coalesce, backfill, skip — got `{}`",
            payload.catch_up
        ))
    })?;
    let trigger =
        service::create_schedule(&state, payload.container_id, payload.cron, catch_up).await?;
    Ok(Success::created(trigger))
}

async fn invoke_webhook(
    State(state): State<AppState>,
    Path(path): Path<String>,
    body: Bytes,
) -> Resp<InvokeOutcome> {
    tracing::info!(%path, "invoke webhook");
    Ok(Success::ok(
        service::handle_webhook(&state, path, body.to_vec()).await?,
    ))
}
