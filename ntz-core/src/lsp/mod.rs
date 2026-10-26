//! Language Server Protocol client: starts language servers for the files being edited and
//! talks JSON-RPC with them over stdio.
//!
//! [`LspManager`] is the entry point: it maps files to servers (by extension), keeps them
//! in sync with the editor's text and collects diagnostics, hover text, completions and
//! definitions.

mod config;
mod framing;
mod manager;
mod position;
mod server;
mod types;
mod uri;

pub use config::{ServerConfig, config_file};
pub use manager::{LspManager, Response};
pub use position::{char_index_to_position, position_to_char_index};
pub use types::{CompletionItem, Diagnostic, Location, Position, Range, RequestId, Severity};
