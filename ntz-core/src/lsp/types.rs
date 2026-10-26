//! The subset of LSP types ntz uses.

use std::path::PathBuf;

/// A position as LSP counts it: zero-based line, and UTF-16 code units within the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    /// Who reported it, e.g. "rustc" or "clippy".
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// Extra info, typically a type or signature.
    pub detail: Option<String>,
    /// Text to insert in place of the word being typed.
    pub insert_text: String,
    /// Exact range the server wants replaced by `insert_text`, if it says.
    pub replace: Option<Range>,
    /// Text to match what the user typed against.
    pub filter_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

/// Identifies a request; its answer comes back from [`super::LspManager::poll`].
pub type RequestId = i64;
