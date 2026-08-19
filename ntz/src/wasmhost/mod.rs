use std::path::Path;

use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{I32Exit, WasiCtxBuilder};

const MAX_CAPTURED_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// One per process. `Engine` compiles and caches wasm — expensive to build,
/// cheap to clone, meant to be shared across every invocation.
pub struct WasmHost {
    engine: Engine,
}

pub struct WasmOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum WasmHostError {
    #[error("wasm setup or trap error: {0}")]
    Wasmtime(#[from] wasmtime::Error),

    #[error("wasm exited with non-zero status {0}")]
    NonZeroExit(i32),
}

impl WasmHost {
    pub fn new() -> Result<Self, WasmHostError> {
        Ok(Self {
            engine: Engine::default(),
        })
    }

    /// A fresh `Store` and `Instance` per call: that's what isolates one
    /// invocation from the next. Only the `Engine`/`Module` are reused.
    pub fn run(&self, wasm_path: &Path, stdin: &[u8]) -> Result<WasmOutcome, WasmHostError> {
        let module = Module::from_file(&self.engine, wasm_path)?;

        let stdout_pipe = MemoryOutputPipe::new(MAX_CAPTURED_OUTPUT_BYTES);
        let stderr_pipe = MemoryOutputPipe::new(MAX_CAPTURED_OUTPUT_BYTES);
        let wasi = WasiCtxBuilder::new()
            .stdin(MemoryInputPipe::new(stdin.to_vec()))
            .stdout(stdout_pipe.clone())
            .stderr(stderr_pipe.clone())
            .build_p1();

        let mut store = Store::new(&self.engine, wasi);
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx| ctx)?;

        let instance = linker.instantiate(&mut store, &module)?;
        let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;

        // A WASI command "returns" by trapping into `proc_exit` — even on
        // success. `I32Exit` carries the real exit code; anything else
        // downcast fails on is a genuine crash/trap.
        match start.call(&mut store, ()) {
            Ok(()) => {}
            Err(err) => match err.downcast::<I32Exit>() {
                Ok(I32Exit(0)) => {}
                Ok(I32Exit(code)) => return Err(WasmHostError::NonZeroExit(code)),
                Err(err) => return Err(WasmHostError::Wasmtime(err)),
            },
        }

        Ok(WasmOutcome {
            stdout: stdout_pipe.contents().to_vec(),
            stderr: stderr_pipe.contents().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_precompiled_go_wasm_module_and_captures_stdout() {
        let host = WasmHost::new().unwrap();
        let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/echo/echo.wasm");

        let outcome = host.run(&wasm_path, b"world").unwrap();

        assert_eq!(outcome.stdout, b"echo: world");
        assert!(outcome.stderr.is_empty());
    }
}
