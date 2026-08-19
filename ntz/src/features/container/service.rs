use chrono::Utc;

use super::{repo, Container, ContainerError, Language};
use crate::db::DbError;
use crate::tools::http::AppError;
use crate::AppState;

pub async fn create(
    state: &AppState,
    name: String,
    language: String,
    source: String,
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

    let now = Utc::now();
    let container = Container {
        id: uuid::Uuid::now_v7().to_string(),
        name,
        language,
        source,
        version: 1,
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
