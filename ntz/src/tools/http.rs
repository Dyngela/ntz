use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use axum::Json;
use http::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::borrow::Cow;
use std::fmt::Write as _;

use crate::features::container::ContainerError;

/// Successful handler output: a status code plus a serializable body.
///
/// This type only ever describes success. Failure is the `Err` side of [`Resp`].
pub struct Success<T> {
    code: StatusCode,
    body: T,
}

impl<T> Success<T> {
    pub fn new(code: StatusCode, body: T) -> Self {
        Self { code, body }
    }

    pub fn ok(body: T) -> Self {
        Self::new(StatusCode::OK, body)
    }

    pub fn created(body: T) -> Self {
        Self::new(StatusCode::CREATED, body)
    }

    pub fn accepted(body: T) -> Self {
        Self::new(StatusCode::ACCEPTED, body)
    }
}

impl<T: Serialize> IntoResponse for Success<T> {
    fn into_response(self) -> Response {
        (self.code, Json(self.body)).into_response()
    }
}

/// What every handler returns. `?` works on any error with a `From` impl below.
pub type Resp<T> = Result<Success<T>, AppError>;

/// The closed set of errors that can reach the HTTP boundary.
///
/// There is deliberately no blanket `From<E>` impl: a foreign error has to be
/// wrapped into a domain error at the point where it happens, while the context
/// is still known. `Internal` is for broken invariants — construct it on
/// purpose, never as a fallback for "some error happened".
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Container(#[from] ContainerError),

    #[error("invalid request: {0}")]
    Validation(String),

    #[error("internal invariant broken: {0}")]
    Internal(Cow<'static, str>),
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        Self::Validation(rejection.body_text())
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    /// Stable, machine-readable. Part of the API contract — never derived from
    /// a variant name, so renaming a variant cannot break a client.
    code: &'static str,
    message: String,
    request_id: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Exhaustive on purpose. Do NOT add a `_ =>` arm: the absence of a
        // wildcard is the only thing that forces a new error to declare its
        // status instead of silently inheriting a fallback.
        let (status, code) = match &self {
            AppError::Container(err) => match err {
                ContainerError::NotFound(_) => (StatusCode::NOT_FOUND, "CONTAINER_NOT_FOUND"),
                ContainerError::NameTaken(_) => (StatusCode::CONFLICT, "CONTAINER_NAME_TAKEN"),
                ContainerError::UnsupportedLanguage(_) => {
                    (StatusCode::UNPROCESSABLE_ENTITY, "UNSUPPORTED_LANGUAGE")
                }
            },
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "INVALID_REQUEST"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL"),
        };

        // The id the request-span is keyed by, so the value handed to the client
        // is the one that greps the log. `-` only if we're outside a request.
        let request_id = crate::tools::telemetry::current_request_id()
            .unwrap_or_else(|| "-".to_owned());

        // Logged once, here, with the whole source chain. Not on the way up.
        // Emitted inside the request span, so it also inherits method and path.
        if status.is_server_error() {
            tracing::error!(%request_id, %code, error = %error_chain(&self), "request failed");
        } else {
            tracing::warn!(%request_id, %code, error = %error_chain(&self), "request rejected");
        }

        // 5xx means we are the broken party: the client gets nothing but the id.
        // 4xx is the client's own input, so echoing the detail is useful.
        let message = if status.is_server_error() {
            "Internal server error".to_owned()
        } else {
            self.to_string()
        };

        let body = ErrorBody {
            error: ErrorDetail {
                code,
                message,
                request_id,
            },
        };

        (status, Json(body)).into_response()
    }
}

/// `a: b: c` for the full `source()` chain.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let _ = write!(out, ": {cause}");
        source = cause.source();
    }
    out
}

/// Drop-in replacement for [`Json`] whose rejection is an [`AppError`], so a
/// malformed body gets the same envelope as every other error instead of
/// axum's default plain-text 400.
pub struct AppJson<T>(pub T);

impl<T, S> FromRequest<S> for AppJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        Ok(Self(value))
    }
}
