use axum::extract::State;
use axum::Router;
use serde::Deserialize;

use super::{service, Container};
use crate::tools::http::{AppJson, Resp, Success};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", axum::routing::get(find_all))
        .route("/create", axum::routing::post(create))
}

#[derive(Deserialize)]
pub struct CreateContainer {
    name: String,
    language: String,
    source: String,
}

async fn create(
    State(state): State<AppState>,
    AppJson(payload): AppJson<CreateContainer>,
) -> Resp<Container> {
    tracing::info!(name = %payload.name, "create container");
    let container = service::create(&state, payload.name, payload.language, payload.source).await?;
    Ok(Success::created(container))
}

async fn find_all(State(state): State<AppState>) -> Resp<Vec<Container>> {
    Ok(Success::ok(service::find_all(&state).await?))
}

async fn update() {}
async fn find_one() {}
async fn delete() {}