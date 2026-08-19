use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Router;
use serde::Deserialize;

use super::{service, Container, InvokeOutcome};
use crate::tools::http::{AppJson, Resp, Success};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", axum::routing::get(find_all))
        .route("/create", axum::routing::post(create))
        .route("/{id}/build", axum::routing::post(build))
        .route("/{id}/invoke", axum::routing::post(invoke))
        .route("/{id}/runs", axum::routing::get(list_runs))
}

#[derive(Deserialize)]
pub struct CreateContainer {
    name: String,
    language: String,
    source: String,
    /// Scheduler-only: how a *scheduled* run retries on failure. Ignored
    /// for direct API/webhook invokes. 0 (the default) means no retries.
    #[serde(default)]
    max_retries: i64,
    #[serde(default)]
    retry_backoff_seconds: i64,
}

async fn create(
    State(state): State<AppState>,
    AppJson(payload): AppJson<CreateContainer>,
) -> Resp<Container> {
    tracing::info!(name = %payload.name, "create container");
    let container = service::create(
        &state,
        payload.name,
        payload.language,
        payload.source,
        payload.max_retries,
        payload.retry_backoff_seconds,
    )
    .await?;
    Ok(Success::created(container))
}

async fn find_all(State(state): State<AppState>) -> Resp<Vec<Container>> {
    Ok(Success::ok(service::find_all(&state).await?))
}

async fn build(State(state): State<AppState>, Path(id): Path<String>) -> Resp<Container> {
    tracing::info!(%id, "build container");
    Ok(Success::ok(service::build(&state, id).await?))
}

/// The request body — any content-type, any shape — goes to the wasm
/// module's stdin verbatim. Same raw-bytes contract webhook invokes use, so
/// both trigger kinds share one code path underneath.
async fn invoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Resp<InvokeOutcome> {
    tracing::info!(%id, "invoke container");
    Ok(Success::ok(
        service::invoke(&state, id, body.to_vec(), None).await?,
    ))
}

async fn list_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Resp<Vec<crate::features::run::Run>> {
    Ok(Success::ok(service::list_runs(&state, id).await?))
}

async fn update() {}
async fn find_one() {}
async fn delete() {}