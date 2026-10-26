use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::config::{self, ServerConfig, language_id};
use super::server::{Incoming, LanguageServer, RequestKind, position_json};
use super::types::{CompletionItem, Diagnostic, Location, Position, RequestId};
use super::uri::{normalize, path_to_uri};

/// An answer to [`LspManager::hover`], [`LspManager::completion`] or
/// [`LspManager::definition`], identified by the id that call returned.
pub enum Response {
    Hover {
        id: RequestId,
        text: Option<String>,
    },
    Completion {
        id: RequestId,
        items: Vec<CompletionItem>,
    },
    Definition {
        id: RequestId,
        locations: Vec<Location>,
    },
}

/// Language servers for one project, started on demand by file extension.
pub struct LspManager {
    root: PathBuf,
    configs: Vec<ServerConfig>,
    config_error: Option<String>,
    /// By server name.
    servers: HashMap<String, Slot>,
    documents: HashMap<PathBuf, Document>,
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    next_id: RequestId,
    notify: Arc<dyn Fn() + Send + Sync>,
}

enum Slot {
    Running(Box<LanguageServer>),
    /// Couldn't start; not retried until the app restarts.
    Failed(String),
}

struct Document {
    server: String,
    version: i32,
}

impl LspManager {
    /// `notify` is called from background threads when a server sends something, e.g. to
    /// request a repaint so that [`Self::poll`] runs.
    pub fn new(project_root: PathBuf, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let (configs, config_error) = config::load(&project_root);
        Self {
            root: project_root,
            configs,
            config_error,
            servers: HashMap::new(),
            documents: HashMap::new(),
            diagnostics: HashMap::new(),
            next_id: 1,
            notify: Arc::new(notify),
        }
    }

    pub fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }

    /// The file was opened in the editor with `text`. Starts its server if needed.
    pub fn open(&mut self, path: &Path, text: &str) {
        let path = normalize(path);
        let Some(config) = self.configs.iter().find(|c| c.handles(&path)).cloned() else {
            return;
        };
        if !self.servers.contains_key(&config.name) {
            let id = self.next_id();
            let slot = match LanguageServer::spawn(&config, &self.root, id, self.notify.clone()) {
                Ok(server) => Slot::Running(Box::new(server)),
                Err(e) => Slot::Failed(e),
            };
            self.servers.insert(config.name.clone(), slot);
        }
        if let Some(Slot::Running(server)) = self.servers.get_mut(&config.name) {
            server.notify(
                "textDocument/didOpen",
                json!({ "textDocument": {
                    "uri": path_to_uri(&path),
                    "languageId": language_id(&path),
                    "version": 1,
                    "text": text,
                } }),
            );
        }
        self.documents.insert(
            path,
            Document {
                server: config.name,
                version: 1,
            },
        );
    }

    /// The file's text is now `text` (the whole of it).
    pub fn change(&mut self, path: &Path, text: &str) {
        let path = normalize(path);
        let Some(document) = self.documents.get_mut(&path) else {
            return;
        };
        document.version += 1;
        let version = document.version;
        if let Some(server) = running(&mut self.servers, &document.server) {
            server.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": path_to_uri(&path), "version": version },
                    "contentChanges": [{ "text": text }],
                }),
            );
        }
    }

    pub fn save(&mut self, path: &Path) {
        self.document_notification(path, "textDocument/didSave");
    }

    pub fn close(&mut self, path: &Path) {
        self.document_notification(path, "textDocument/didClose");
        let path = normalize(path);
        self.documents.remove(&path);
        self.diagnostics.remove(&path);
    }

    pub fn hover(&mut self, path: &Path, position: Position) -> Option<RequestId> {
        self.position_request(path, position, RequestKind::Hover, "textDocument/hover")
    }

    pub fn completion(&mut self, path: &Path, position: Position) -> Option<RequestId> {
        self.position_request(
            path,
            position,
            RequestKind::Completion,
            "textDocument/completion",
        )
    }

    pub fn definition(&mut self, path: &Path, position: Position) -> Option<RequestId> {
        self.position_request(
            path,
            position,
            RequestKind::Definition,
            "textDocument/definition",
        )
    }

    /// Handles what the servers sent; returns the answers to requests.
    pub fn poll(&mut self) -> Vec<Response> {
        let mut responses = Vec::new();
        for slot in self.servers.values_mut() {
            let Slot::Running(server) = slot else {
                continue;
            };
            for incoming in server.poll() {
                match incoming {
                    Incoming::Diagnostics(path, diagnostics) => {
                        self.diagnostics.insert(path, diagnostics);
                    }
                    Incoming::Hover(id, text) => responses.push(Response::Hover { id, text }),
                    Incoming::Completion(id, items) => {
                        responses.push(Response::Completion { id, items })
                    }
                    Incoming::Definition(id, locations) => {
                        responses.push(Response::Definition { id, locations })
                    }
                }
            }
            if !server.is_alive() {
                *slot = Slot::Failed(server.status());
            }
        }
        responses
    }

    pub fn diagnostics(&self, path: &Path) -> &[Diagnostic] {
        self.diagnostics
            .get(&normalize(path))
            .map_or(&[], Vec::as_slice)
    }

    /// "rust-analyzer: ready", "rust-analyzer: Indexing 40%"… for the file's server.
    pub fn status(&self, path: &Path) -> Option<String> {
        let path = normalize(path);
        let name = match self.documents.get(&path) {
            Some(document) => &document.server,
            None => &self.configs.iter().find(|c| c.handles(&path))?.name,
        };
        let status = match self.servers.get(name)? {
            Slot::Running(server) => server.status(),
            Slot::Failed(reason) => reason.clone(),
        };
        Some(format!("{name}: {status}"))
    }

    fn document_notification(&mut self, path: &Path, method: &str) {
        let path = normalize(path);
        let Some(document) = self.documents.get(&path) else {
            return;
        };
        if let Some(server) = running(&mut self.servers, &document.server) {
            server.notify(
                method,
                json!({ "textDocument": { "uri": path_to_uri(&path) } }),
            );
        }
    }

    fn position_request(
        &mut self,
        path: &Path,
        position: Position,
        kind: RequestKind,
        method: &str,
    ) -> Option<RequestId> {
        let path = normalize(path);
        let server = self.documents.get(&path)?.server.clone();
        let id = self.next_id();
        let server = running(&mut self.servers, &server)?;
        server.request(
            id,
            kind,
            method,
            json!({
                "textDocument": { "uri": path_to_uri(&path) },
                "position": position_json(position),
            }),
        );
        Some(id)
    }

    fn next_id(&mut self) -> RequestId {
        self.next_id += 1;
        self.next_id
    }
}

fn running<'a>(
    servers: &'a mut HashMap<String, Slot>,
    name: &str,
) -> Option<&'a mut LanguageServer> {
    match servers.get_mut(name)? {
        Slot::Running(server) if server.is_alive() => Some(server),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::lsp::Severity;

    /// Drives the real rust-analyzer on this workspace: diagnostics for a broken file, then
    /// hover, completion and go-to-definition. Slow (indexing), so not run by default.
    #[test]
    #[ignore = "starts rust-analyzer on this workspace (slow)"]
    fn rust_analyzer_end_to_end() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let mut lsp = LspManager::new(root.clone(), || {});
        let path = root.join("ntz-core").join("src").join("lib.rs");
        let text = std::fs::read_to_string(&path).unwrap();
        // A syntax error: rust-analyzer reports those live (type errors only come from
        // `cargo check`, after a save).
        let broken = format!("{text}\nfn broken() {{ let x = ; }}\n");
        lsp.open(&path, &broken);

        let wait =
            |lsp: &mut LspManager,
             what: &str,
             done: &mut dyn FnMut(&mut LspManager, Vec<Response>) -> bool| {
                let deadline = Instant::now() + Duration::from_secs(180);
                loop {
                    let responses = lsp.poll();
                    if done(lsp, responses) {
                        return;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for {what}: {:?}",
                        lsp.status(&path)
                    );
                    std::thread::sleep(Duration::from_millis(100));
                }
            };

        let broken_line = broken.lines().count() as u32 - 1;
        wait(&mut lsp, "diagnostics", &mut |lsp, _| {
            lsp.diagnostics(&path)
                .iter()
                .any(|d| d.range.start.line == broken_line && d.severity == Severity::Error)
        });

        // `add` is defined in lib.rs: `pub fn add(left: u64, …`
        let add = broken.find("add(").unwrap();
        let add_position = super::super::char_index_to_position(&broken, add + 1);
        let hover = lsp.hover(&path, add_position).unwrap();
        wait(&mut lsp, "hover", &mut |_, responses| {
            responses.iter().any(|r| matches!(r, Response::Hover { id, text: Some(text) } if *id == hover && text.contains("fn add")))
        });

        let definition = lsp.definition(&path, add_position).unwrap();
        wait(&mut lsp, "definition", &mut |_, responses| {
            responses.iter().any(|r| matches!(r, Response::Definition { id, locations } if *id == definition && locations.iter().any(|l| l.path == normalize(&path))))
        });

        let with_dot = format!("{broken}\nfn uses() {{ let s = String::new(); s. }}\n");
        lsp.change(&path, &with_dot);
        let dot = with_dot.rfind("s.").unwrap() + 2;
        let dot = super::super::char_index_to_position(&with_dot, dot);
        // Right after an edit the server may still be re-analyzing and answer nothing (an
        // editor just asks again as the user keeps typing): retry until it has the answer.
        let mut completion = lsp.completion(&path, dot).unwrap();
        wait(&mut lsp, "completion", &mut |lsp, responses| {
            for response in responses {
                if let Response::Completion { id, items } = response
                    && id == completion
                {
                    if items.iter().any(|i| i.label.starts_with("len")) {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    completion = lsp.completion(&path, dot).unwrap();
                }
            }
            false
        });
    }
}
