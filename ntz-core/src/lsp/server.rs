//! One running language server: the process, the JSON-RPC exchange and the parsing of
//! what it answers.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::config::{ServerConfig, resolve_program};
use super::framing::{read_message, write_message};
use super::types::{CompletionItem, Diagnostic, Location, Position, Range, RequestId, Severity};
use super::uri::{path_to_uri, uri_to_path};
use crate::run::job::Job;
use crate::run::process::hide_console;

/// What a request was for, to parse its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Hover,
    Completion,
    Definition,
}

/// What the server sent that the manager cares about.
pub enum Incoming {
    Diagnostics(PathBuf, Vec<Diagnostic>),
    Hover(RequestId, Option<String>),
    Completion(RequestId, Vec<CompletionItem>),
    Definition(RequestId, Vec<Location>),
}

enum State {
    Initializing,
    Ready,
    Exited(String),
}

enum Pending {
    Initialize,
    Request(RequestKind),
}

/// Work in progress the server reports (e.g. rust-analyzer indexing).
struct Progress {
    title: String,
    percentage: Option<u64>,
}

pub struct LanguageServer {
    child: Child,
    /// Kills the server (and anything it started) when dropped.
    _job: Option<Job>,
    outgoing: Sender<Value>,
    incoming: Receiver<Value>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    state: State,
    pending: HashMap<RequestId, Pending>,
    /// Messages to send once the server is initialized.
    queued: Vec<Value>,
    progress: HashMap<String, Progress>,
}

impl LanguageServer {
    pub fn spawn(
        config: &ServerConfig,
        root: &Path,
        initialize_id: RequestId,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let program = resolve_program(&config.command);
        let mut command = Command::new(&program);
        command
            .args(&config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut command);
        let mut child = command.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{} not found: install it, or set its path in .ntz/lsp.json",
                config.command
            ),
            _ => format!("{}: {e}", config.command),
        })?;
        let job = Job::new().ok().filter(|job| job.assign(&child).is_ok());

        let (outgoing, to_write) = mpsc::channel::<Value>();
        let mut stdin = child.stdin.take().expect("piped");
        std::thread::spawn(move || {
            for message in to_write {
                if write_message(&mut stdin, &message).is_err() {
                    break;
                }
            }
        });

        let (received, incoming) = mpsc::channel();
        let stdout = child.stdout.take().expect("piped");
        let reader_notify = notify.clone();
        std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut stdout) {
                if received.send(message).is_err() {
                    break;
                }
                reader_notify();
            }
            reader_notify(); // Lets the manager notice the exit.
        });

        // Kept to explain an early exit (e.g. rustup: "Unknown binary 'rust-analyzer'").
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = Arc::clone(&stderr_tail);
        let stderr = child.stderr.take().expect("piped");
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut tail = tail.lock().unwrap();
                if tail.len() == 20 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        let mut server = Self {
            child,
            _job: job,
            outgoing,
            incoming,
            stderr_tail,
            state: State::Initializing,
            pending: HashMap::new(),
            queued: Vec::new(),
            progress: HashMap::new(),
        };
        server.pending.insert(initialize_id, Pending::Initialize);
        server.send(json!({
            "jsonrpc": "2.0",
            "id": initialize_id,
            "method": "initialize",
            "params": initialize_params(root),
        }));
        Ok(server)
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send_when_ready(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn request(&mut self, id: RequestId, kind: RequestKind, method: &str, params: Value) {
        self.pending.insert(id, Pending::Request(kind));
        self.send_when_ready(
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
    }

    pub fn is_alive(&self) -> bool {
        !matches!(self.state, State::Exited(_))
    }

    /// "ready", "starting…", "Indexing 40%", or why it stopped.
    pub fn status(&self) -> String {
        match &self.state {
            State::Initializing => "starting…".to_owned(),
            State::Exited(reason) => reason.clone(),
            State::Ready => match self.progress.values().next() {
                Some(Progress {
                    title,
                    percentage: Some(percentage),
                }) => format!("{title} {percentage}%"),
                Some(Progress { title, .. }) => format!("{title}…"),
                None => "ready".to_owned(),
            },
        }
    }

    /// Handles everything the server sent since the last call.
    pub fn poll(&mut self) -> Vec<Incoming> {
        let mut out = Vec::new();
        loop {
            match self.incoming.try_recv() {
                Ok(message) => self.handle(message, &mut out),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.is_alive() {
                        self.state = State::Exited(self.exit_reason());
                    }
                    break;
                }
            }
        }
        out
    }

    fn handle(&mut self, message: Value, out: &mut Vec<Incoming>) {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match (id, method) {
            // Answer to one of our requests.
            (Some(id), None) => {
                let Some(pending) = id.as_i64().and_then(|id| self.pending.remove(&id)) else {
                    return;
                };
                let id = id.as_i64().unwrap_or_default();
                let result = message.get("result").cloned().unwrap_or(Value::Null);
                match pending {
                    Pending::Initialize => {
                        self.state = State::Ready;
                        self.send(
                            json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
                        );
                        for message in std::mem::take(&mut self.queued) {
                            self.send(message);
                        }
                    }
                    Pending::Request(RequestKind::Hover) => {
                        out.push(Incoming::Hover(id, parse_hover(&result)))
                    }
                    Pending::Request(RequestKind::Completion) => {
                        out.push(Incoming::Completion(id, parse_completion(&result)))
                    }
                    Pending::Request(RequestKind::Definition) => {
                        out.push(Incoming::Definition(id, parse_locations(&result)))
                    }
                }
            }
            // The server asks us something: answer so it doesn't wait forever.
            (Some(id), Some(method)) => {
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; items])
                    }
                    _ => Value::Null,
                };
                self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            (None, Some(method)) => self.handle_notification(&method, &message["params"], out),
            (None, None) => {}
        }
    }

    fn handle_notification(&mut self, method: &str, params: &Value, out: &mut Vec<Incoming>) {
        match method {
            "textDocument/publishDiagnostics" => {
                let Some(path) = params["uri"].as_str().and_then(uri_to_path) else {
                    return;
                };
                let diagnostics = params["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(parse_diagnostic)
                    .collect();
                out.push(Incoming::Diagnostics(path, diagnostics));
            }
            "$/progress" => {
                let token = params["token"].to_string();
                let value = &params["value"];
                match value["kind"].as_str() {
                    Some("begin") => {
                        let title = value["title"].as_str().unwrap_or("Working").to_owned();
                        let percentage = value["percentage"].as_u64();
                        self.progress.insert(token, Progress { title, percentage });
                    }
                    Some("report") => {
                        if let Some(progress) = self.progress.get_mut(&token) {
                            progress.percentage =
                                value["percentage"].as_u64().or(progress.percentage);
                        }
                    }
                    Some("end") => {
                        self.progress.remove(&token);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn send_when_ready(&mut self, message: Value) {
        match self.state {
            State::Ready => self.send(message),
            State::Initializing => self.queued.push(message),
            State::Exited(_) => {}
        }
    }

    fn send(&self, message: Value) {
        let _ = self.outgoing.send(message);
    }

    fn exit_reason(&mut self) -> String {
        let code = self.child.try_wait().ok().flatten().and_then(|s| s.code());
        let tail = self.stderr_tail.lock().unwrap();
        let last = tail.iter().rev().find(|l| !l.trim().is_empty());
        match (last, code) {
            (Some(line), _) => format!("stopped: {}", line.trim()),
            (None, Some(code)) => format!("stopped (exit code {code})"),
            (None, None) => "stopped".to_owned(),
        }
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        // Polite exit; the job kills it anyway when dropped right after.
        self.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
    }
}

fn initialize_params(root: &Path) -> Value {
    let root_uri = path_to_uri(root);
    let name = root
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "ntz" },
        "rootUri": root_uri,
        "workspaceFolders": [{ "uri": root_uri, "name": name }],
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "window": { "workDoneProgress": true },
            "textDocument": {
                "synchronization": { "didSave": true },
                "publishDiagnostics": {},
                "hover": { "contentFormat": ["plaintext", "markdown"] },
                "completion": { "completionItem": { "snippetSupport": false } },
                "definition": { "linkSupport": true },
            },
        },
    })
}

pub fn position_json(position: Position) -> Value {
    json!({ "line": position.line, "character": position.character })
}

fn parse_position(value: &Value) -> Option<Position> {
    Some(Position {
        line: value["line"].as_u64()? as u32,
        character: value["character"].as_u64()? as u32,
    })
}

fn parse_range(value: &Value) -> Option<Range> {
    Some(Range {
        start: parse_position(&value["start"])?,
        end: parse_position(&value["end"])?,
    })
}

fn parse_diagnostic(value: &Value) -> Option<Diagnostic> {
    Some(Diagnostic {
        range: parse_range(&value["range"])?,
        severity: match value["severity"].as_u64() {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Information,
            Some(4) => Severity::Hint,
            _ => Severity::Error,
        },
        message: value["message"].as_str()?.to_owned(),
        source: value["source"].as_str().map(str::to_owned),
    })
}

/// Hover contents as plain text (markdown code fences dropped).
fn parse_hover(result: &Value) -> Option<String> {
    fn text(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Array(items) => {
                let parts: Vec<_> = items.iter().filter_map(text).collect();
                (!parts.is_empty()).then(|| parts.join("\n\n"))
            }
            Value::Object(_) => value["value"].as_str().map(str::to_owned),
            _ => None,
        }
    }
    let text = text(&result["contents"])?;
    let text: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect();
    let text = text.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn parse_completion(result: &Value) -> Vec<CompletionItem> {
    let items = match result {
        Value::Array(items) => items,
        _ => match result["items"].as_array() {
            Some(items) => items,
            None => return Vec::new(),
        },
    };
    let mut parsed: Vec<(String, CompletionItem)> = items
        .iter()
        .filter_map(|item| {
            let label = item["label"].as_str()?.to_owned();
            let edit = &item["textEdit"];
            let (new_text, replace) = match edit["newText"].as_str() {
                // A plain edit has `range`; an insert/replace edit has `insert`.
                Some(text) => (
                    Some(text),
                    parse_range(&edit["range"]).or(parse_range(&edit["insert"])),
                ),
                None => (None, None),
            };
            let insert_text = new_text
                .or(item["insertText"].as_str())
                .unwrap_or(&label)
                .to_owned();
            let insert_text = if item["insertTextFormat"].as_u64() == Some(2) {
                strip_snippet(&insert_text)
            } else {
                insert_text
            };
            let sort = item["sortText"].as_str().unwrap_or(&label).to_owned();
            Some((
                sort,
                CompletionItem {
                    filter_text: item["filterText"].as_str().unwrap_or(&label).to_owned(),
                    detail: item["detail"].as_str().map(str::to_owned),
                    label,
                    insert_text,
                    replace,
                },
            ))
        })
        .collect();
    parsed.sort_by(|a, b| a.0.cmp(&b.0));
    parsed.into_iter().map(|(_, item)| item).collect()
}

/// `foo(${1:x}, $2)$0` → `foo(x, )`: placeholders become their default text.
fn strip_snippet(snippet: &str) -> String {
    let mut out = String::new();
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
                if chars.peek() == Some(&':') {
                    chars.next();
                }
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    out.push(c);
                }
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn parse_locations(result: &Value) -> Vec<Location> {
    let one = |value: &Value| -> Option<Location> {
        // A `Location` has `uri`/`range`; a `LocationLink` has `targetUri`/`targetSelectionRange`.
        let uri = value["uri"].as_str().or(value["targetUri"].as_str())?;
        let range = parse_range(&value["range"]).or(parse_range(&value["targetSelectionRange"]))?;
        Some(Location {
            path: uri_to_path(uri)?,
            range,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_become_plain_text() {
        assert_eq!(strip_snippet("foo(${1:x}, $2)$0"), "foo(x, )");
        assert_eq!(strip_snippet(r"cost \$5"), "cost $5");
    }

    #[test]
    fn completion_list_is_sorted_and_uses_text_edits() {
        let result = json!({ "items": [
            { "label": "zeta", "sortText": "2" },
            { "label": "alpha", "sortText": "1", "detail": "fn()",
              "textEdit": { "newText": "alpha()", "range": {
                  "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 6 } } } }
        ] });
        let items = parse_completion(&result);
        assert_eq!(items[0].label, "alpha");
        assert_eq!(items[0].insert_text, "alpha()");
        assert_eq!(items[0].replace.unwrap().start.character, 4);
        assert_eq!(items[1].insert_text, "zeta");
    }

    #[test]
    fn hover_markdown_is_flattened() {
        let result = json!({ "contents": { "kind": "markdown",
            "value": "```rust\nfn main()\n```\n\nThe entry point." } });
        assert_eq!(
            parse_hover(&result).unwrap(),
            "fn main()\n\nThe entry point."
        );
    }
}
