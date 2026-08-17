use std::io::IsTerminal;
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use tracing::Instrument;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

const REQUEST_ID_HEADER: &str = "x-request-id";

/// Must stay alive for the whole process: dropping it flushes and stops the
/// background writer threads. Losing it silently discards buffered logs.
pub struct LogGuards(#[allow(dead_code)] Vec<WorkerGuard>);

/// `NTZ_LOG` / `RUST_LOG` control verbosity, `NTZ_LOG_FORMAT=json` switches the
/// console to machine-readable, `NTZ_LOG_DIR` adds a daily-rotated JSON file.
pub fn init() -> anyhow::Result<LogGuards> {
    let mut guards = Vec::new();
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();

    let (console_writer, console_guard) = tracing_appender::non_blocking(std::io::stdout());
    guards.push(console_guard);

    if json_console() {
        layers.push(Box::new(
            json_layer().with_writer(console_writer).with_filter(filter()),
        ));
    } else {
        layers.push(Box::new(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                // Escape codes are noise once stdout is redirected to a file,
                // which is exactly what a Windows service does.
                .with_ansi(std::io::stdout().is_terminal())
                .with_writer(console_writer)
                .with_filter(filter()),
        ));
    }

    if let Some(dir) = std::env::var_os("NTZ_LOG_DIR") {
        // `rolling::daily` never deletes anything. Bounded retention instead, or
        // the disk fills months later on a machine nobody is watching.
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("ntz")
            .filename_suffix("log")
            .max_log_files(retained_log_files())
            .build(dir)?;
        let (file_writer, file_guard) = tracing_appender::non_blocking(appender);
        guards.push(file_guard);
        layers.push(Box::new(
            json_layer().with_writer(file_writer).with_filter(filter()),
        ));
    }

    tracing_subscriber::registry().with(layers).try_init()?;

    Ok(LogGuards(guards))
}

fn retained_log_files() -> usize {
    std::env::var("NTZ_LOG_RETAIN_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14)
}

fn json_console() -> bool {
    matches!(
        std::env::var("NTZ_LOG_FORMAT").as_deref(),
        Ok("json") | Ok("JSON")
    )
}

fn json_layer<S>() -> tracing_subscriber::fmt::Layer<
    S,
    tracing_subscriber::fmt::format::JsonFields,
    tracing_subscriber::fmt::format::Format<tracing_subscriber::fmt::format::Json>,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer()
        .json()
        .flatten_event(true)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        // The two that make span context usable downstream: the enclosing span's
        // fields, and the full ancestor chain.
        .with_current_span(true)
        .with_span_list(true)
}

/// `EnvFilter` isn't `Clone`, so each layer gets its own built from the same source.
fn filter() -> EnvFilter {
    EnvFilter::try_from_env("NTZ_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,ntz=debug"))
}

tokio::task_local! {
    static REQUEST_ID: String;
}

/// The id of the request being served, if called from within one. Lets the error
/// boundary put the *same* id in the response body that the logs are keyed by.
pub fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(String::clone).ok()
}

/// Wraps every request in a span carrying its id, method and path, so all events
/// emitted downstream inherit them, and emits one completion event with status
/// and latency.
pub async fn trace_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let request_id = inbound_request_id(&req).unwrap_or_else(new_request_id);

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
    );

    let started = Instant::now();
    let mut response = REQUEST_ID
        .scope(request_id.clone(), next.run(req))
        .instrument(span.clone())
        .await;
    let latency = started.elapsed();

    // Emitted *inside* the span, not with `parent:`. Both attribute the event
    // correctly, but `with_span_list` walks the current context, so `parent:`
    // alone yields `"spans": []` and a log backend loses the ancestor chain.
    span.in_scope(|| {
        tracing::info!(
            status = response.status().as_u16(),
            // `as_millis()` is u128, which tracing's serde bridge cannot
            // represent — it degrades to a *string*, so latency stops being
            // aggregatable. u64 milliseconds stays a JSON number.
            latency_ms = latency.as_millis() as u64,
            "request completed"
        );
    });

    // Echoed on success too, so a client can quote it in a bug report.
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }

    response
}

fn new_request_id() -> String {
    // v7 is time-ordered: ids sort chronologically, which makes them pleasant
    // to scan in a log file and cheap to index.
    uuid::Uuid::now_v7().to_string()
}

/// Reuses a caller-supplied id so a trace spans several services — but only if
/// it's harmless. An unvalidated header lands verbatim in the log file, where a
/// newline lets a caller forge entries.
fn inbound_request_id(req: &Request) -> Option<String> {
    let raw = req.headers().get(REQUEST_ID_HEADER)?.to_str().ok()?;

    let acceptable = !raw.is_empty()
        && raw.len() <= 128
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));

    acceptable.then(|| raw.to_owned())
}
