use std::future::Future;

/// Resolves when the OS asks us to stop.
///
/// Handlers are installed eagerly so a failure surfaces at startup rather than
/// at shutdown, when it is too late to do anything about it.
#[cfg(windows)]
pub fn signal() -> anyhow::Result<impl Future<Output = ()>> {
    use tokio::signal::windows;

    let mut ctrl_c = windows::ctrl_c()?;
    // Console window closed, and the OS reboot/logoff notification. Windows
    // gives roughly five seconds after these before killing us outright.
    let mut ctrl_close = windows::ctrl_close()?;
    let mut ctrl_shutdown = windows::ctrl_shutdown()?;

    Ok(async move {
        let reason = tokio::select! {
            _ = ctrl_c.recv() => "ctrl_c",
            _ = ctrl_close.recv() => "ctrl_close",
            _ = ctrl_shutdown.recv() => "ctrl_shutdown",
        };
        tracing::info!(signal = reason, "shutdown requested");
    })
}

#[cfg(unix)]
pub fn signal() -> anyhow::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;

    Ok(async move {
        let reason = tokio::select! {
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        };
        tracing::info!(signal = reason, "shutdown requested");
    })
}
