pub mod config;
pub mod db;
pub mod features;
pub mod tools;
pub mod toolchain;
pub mod wasmhost;

use std::sync::Arc;

use axum::Router;
use http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;

use config::Config;
use db::Db;
use toolchain::Toolchain;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub config: Arc<Config>,
    pub toolchain: Arc<Toolchain>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _log_guards = tools::telemetry::init()?;

    let config = Config::from_env();
    let db = Db::open(&config.db_path)?;
    // Each feature owns and applies its own table(s) — the db module only
    // hands out the connection.
    features::container::repo::migrate(&db)?;
    let port = config.port;
    let toolchain = Toolchain::new(config.toolchain_dir.clone());
    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(config),
        toolchain: Arc::new(toolchain),
    };

    let app = Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .nest("/containers", features::container::handler::router())
        .layer(
            CorsLayer::new()
                .allow_origin("*".parse::<HeaderValue>()?)
                .allow_methods([Method::GET, Method::POST]),
        )
        // Outermost, so even a rejected CORS preflight gets traced.
        .layer(axum::middleware::from_fn(tools::telemetry::trace_request))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("listening on {}", listener.local_addr()?);

    // Graceful shutdown belongs to the server, not the router: it lets in-flight
    // requests finish, then returns so `_log_guards` drops and flushes.
    axum::serve(listener, app)
        .with_graceful_shutdown(tools::shutdown::signal()?)
        .await?;

    tracing::info!("stopped");
    Ok(())
}
