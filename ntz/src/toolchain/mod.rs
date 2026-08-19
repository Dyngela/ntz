use std::path::{Path, PathBuf};

use tokio::process::Command;
use tokio::sync::OnceCell;

// Pinned, not "latest": a build that worked yesterday must still work
// tomorrow. Bumping any of these is a deliberate, tested decision, same
// rationale as `rust-toolchain.toml`.
const GO_URL: &str = "https://go.dev/dl/go1.25.1.windows-amd64.zip";
const TINYGO_URL: &str =
    "https://github.com/tinygo-org/tinygo/releases/download/v0.41.1/tinygo0.41.1.windows-amd64.zip";
const BINARYEN_URL: &str = "https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-windows.tar.gz";

/// Resolved locations of every component the Go build pipeline needs.
pub struct ToolchainPaths {
    pub go_root: PathBuf,
    pub tinygo_root: PathBuf,
    pub wasm_opt: PathBuf,
}

/// Provisions Go + TinyGo + wasm-opt into `dir` on first use, then reuses
/// them from disk. Nobody installs anything by hand: the first Go build
/// pays a one-time download, every build after that is fully offline.
pub struct Toolchain {
    dir: PathBuf,
    ready: OnceCell<ToolchainPaths>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("download of {0} failed (curl exit {1:?})")]
    DownloadFailed(&'static str, Option<i32>),

    #[error("extraction of {0} failed (tar exit {1:?})")]
    ExtractFailed(&'static str, Option<i32>),

    #[error("tinygo build failed (exit {0:?}): {1}")]
    BuildFailed(Option<i32>, String),
}

impl Toolchain {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            ready: OnceCell::new(),
        }
    }

    /// Downloads and extracts whatever components are missing, then returns
    /// their paths. Concurrent callers await the same in-flight work instead
    /// of racing each other into duplicate downloads.
    pub async fn ensure(&self) -> Result<&ToolchainPaths, ToolchainError> {
        self.ready.get_or_try_init(|| self.provision()).await
    }

    async fn provision(&self) -> Result<ToolchainPaths, ToolchainError> {
        std::fs::create_dir_all(&self.dir)?;

        let go_root = self.dir.join("go");
        if !go_root.join("bin").join("go.exe").exists() {
            fetch_and_extract("go", GO_URL, &go_root).await?;
        }

        let tinygo_root = self.dir.join("tinygo");
        if !tinygo_root.join("bin").join("tinygo.exe").exists() {
            fetch_and_extract("tinygo", TINYGO_URL, &tinygo_root).await?;
        }

        let binaryen_root = self.dir.join("binaryen");
        let wasm_opt = binaryen_root.join("bin").join("wasm-opt.exe");
        if !wasm_opt.exists() {
            fetch_and_extract("binaryen", BINARYEN_URL, &binaryen_root).await?;
        }

        Ok(ToolchainPaths {
            go_root,
            tinygo_root,
            wasm_opt,
        })
    }

    /// Compiles `source` (a single `main.go`) to a WASI preview1 module at
    /// `out_path`, provisioning the toolchain first if needed.
    pub async fn build_go_wasm(&self, source: &str, out_path: &Path) -> Result<(), ToolchainError> {
        let paths = self.ensure().await?;

        let work_dir = std::env::temp_dir().join(format!("ntz-build-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&work_dir)?;
        let main_go = work_dir.join("main.go");
        std::fs::write(&main_go, source)?;

        // TinyGo shells out to `go` itself, so both bin dirs need to be on
        // PATH, not just pointed at via GOROOT.
        let path = format!(
            "{};{};{}",
            paths.tinygo_root.join("bin").display(),
            paths.go_root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default(),
        );

        let output = Command::new(paths.tinygo_root.join("bin").join("tinygo.exe"))
            .arg("build")
            .arg("-o")
            .arg(out_path)
            .arg("-target=wasip1")
            .arg(&main_go)
            .env("PATH", path)
            .env("GOROOT", &paths.go_root)
            .env("WASMOPT", &paths.wasm_opt)
            .output()
            .await?;

        let _ = std::fs::remove_dir_all(&work_dir);

        if !output.status.success() {
            return Err(ToolchainError::BuildFailed(
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(())
    }
}

/// Downloads `url` and extracts it into `target_dir`, stripping the
/// archive's own top-level wrapper folder (`go/`, `tinygo/`,
/// `binaryen-version_NNN/`, ...) so callers get a stable layout regardless
/// of how upstream names that folder.
async fn fetch_and_extract(
    name: &'static str,
    url: &str,
    target_dir: &Path,
) -> Result<(), ToolchainError> {
    std::fs::create_dir_all(target_dir)?;
    let archive_path = target_dir.with_extension("download");

    tracing::info!(component = name, url, "downloading toolchain component");
    let status = Command::new("curl")
        .arg("-fsSL")
        .arg("-o")
        .arg(&archive_path)
        .arg(url)
        .status()
        .await?;
    if !status.success() {
        return Err(ToolchainError::DownloadFailed(name, status.code()));
    }

    tracing::info!(component = name, "extracting toolchain component");
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive_path)
        .arg("-C")
        .arg(target_dir)
        .arg("--strip-components=1")
        .status()
        .await?;
    let _ = std::fs::remove_file(&archive_path);
    if !status.success() {
        return Err(ToolchainError::ExtractFailed(name, status.code()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "downloads ~340MB on first run; run explicitly with `cargo test -- --ignored`"]
    async fn self_bootstraps_into_a_fresh_directory_and_compiles_go_to_wasm() {
        // A directory that has never seen Go/TinyGo/Binaryen before — proves
        // this is a real bootstrap, not just "reuses what's on this machine".
        let scratch =
            std::env::temp_dir().join(format!("ntz-toolchain-test-{}", uuid::Uuid::now_v7()));
        let toolchain = Toolchain::new(scratch.clone());

        let source = include_str!("../../fixtures/echo/main.go");
        let out_path = scratch.join("echo.wasm");
        toolchain.build_go_wasm(source, &out_path).await.unwrap();

        // `WasmHost::run` bridges through wasmtime-wasi's sync WASI shim,
        // which calls `Handle::block_on` internally. Called directly from
        // this `async fn` (already running on a tokio-driven thread) that
        // panics with "cannot start a runtime from within a runtime" —
        // `spawn_blocking` moves it off the async task-polling thread.
        let outcome = tokio::task::spawn_blocking(move || {
            let host = crate::wasmhost::WasmHost::new().unwrap();
            host.run(&out_path, b"world")
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(outcome.stdout, b"echo: world");

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
