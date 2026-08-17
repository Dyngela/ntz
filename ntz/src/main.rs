pub mod features;
pub mod tools;

use axum::Router;
use http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;

#[derive(Default, Clone)]
pub struct AppState {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _log_guards = tools::telemetry::init()?;

    let state = AppState::default();
    let app = Router::new()
        .nest("/containers", features::container::handler::router())
        .layer(
            CorsLayer::new()
                .allow_origin("*".parse::<HeaderValue>()?)
                .allow_methods([Method::GET, Method::POST]),
        )
        // Outermost, so even a rejected CORS preflight gets traced.
        .layer(axum::middleware::from_fn(tools::telemetry::trace_request))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    // Graceful shutdown belongs to the server, not the router: it lets in-flight
    // requests finish, then returns so `_log_guards` drops and flushes.
    axum::serve(listener, app)
        .with_graceful_shutdown(tools::shutdown::signal()?)
        .await?;

    tracing::info!("stopped");
    Ok(())
}
