use axum::Router;
use serde::{Deserialize, Serialize};

use crate::features::container::Language;
use crate::tools::http::{AppError, AppJson, Resp, Success};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/create", axum::routing::post(create))
}

#[derive(Deserialize)]
pub struct CreateContainer {
    name: String,
    language: String,
}

#[derive(Serialize)]
pub struct CreateContainerResp {
    name: String,
    language: String,
}

async fn create(AppJson(payload): AppJson<CreateContainer>) -> Resp<CreateContainerResp> {
    tracing::info!(name = %payload.name, "create container");
    let language = Language::parse(&payload.language)?;

    if payload.name.trim().is_empty() {
        return Err(AppError::Validation("name must not be empty".to_owned()));
    }

    Ok(Success::created(CreateContainerResp {
        name: payload.name,
        language: language.as_str().to_owned(),
    }))
}

async fn update() {}
async fn find_all() {}
async fn find_one() {}
async fn delete() {}