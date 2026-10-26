//! Central editor area: a tab per open file on top, the active file's text below, with
//! language-server features (diagnostics, hover, completion, go to definition).

mod code_view;
mod completion;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use ntz_core::lsp::{self, LspManager, RequestId, Severity};

use self::completion::{Completion, Key};

/// How long the mouse must rest on a symbol before asking the server about it.
const HOVER_DELAY: Duration = Duration::from_millis(400);
/// An empty completion answer right after an edit usually means the server was still
/// re-analyzing: ask again this many times.
const COMPLETION_RETRIES: u8 = 8;

pub struct Editor {
    documents: Vec<Document>,
    active: Option<usize>,
    lsp: LspManager,
    hover: Hover,
    completion: Option<Completion>,
    completion_request: Option<CompletionRequest>,
    definition_request: Option<RequestId>,
}

pub struct Document {
    pub path: PathBuf,
    pub text: String,
    dirty: bool,
    /// Char index of the cursor (kept when the text loses focus).
    cursor: usize,
    /// Move the cursor here (and scroll to it) next frame.
    jump_to: Option<usize>,
}

/// Hover state: the word under the mouse and what the server said about it.
#[derive(Default)]
struct Hover {
    /// Word start under the mouse, and since when it's there.
    resting: Option<(usize, Instant)>,
    request: Option<(RequestId, usize)>,
    text: Option<(usize, String)>,
}

struct CompletionRequest {
    id: RequestId,
    anchor: usize,
    cursor: usize,
    retries: u8,
    retry_at: Option<Instant>,
}

impl Document {
    fn title(&self) -> String {
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if self.dirty {
            format!("{name} *")
        } else {
            name
        }
    }
}

impl Editor {
    pub fn new(project_root: PathBuf, ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            documents: Vec::new(),
            active: None,
            lsp: LspManager::new(project_root, move || ctx.request_repaint()),
            hover: Hover::default(),
            completion: None,
            completion_request: None,
            definition_request: None,
        }
    }

    /// Focuses the file's tab if already open, otherwise opens it in a new tab.
    pub fn open(&mut self, path: PathBuf) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|d| same_file(&d.path, &path))
        {
            self.active = Some(index);
            return;
        }
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| format!("<cannot open file: {e}>"));
        self.lsp.open(&path, &text);
        self.documents.push(Document {
            path,
            text,
            dirty: false,
            cursor: 0,
            jump_to: None,
        });
        self.active = Some(self.documents.len() - 1);
    }

    pub fn active_document(&self) -> Option<&Document> {
        self.active.map(|i| &self.documents[i])
    }

    pub fn save_active(&mut self) {
        let Some(doc) = self.active.map(|i| &mut self.documents[i]) else {
            return;
        };
        match std::fs::write(&doc.path, &doc.text) {
            Ok(()) => {
                doc.dirty = false;
                self.lsp.save(&doc.path);
            }
            Err(e) => eprintln!("failed to save {}: {e}", doc.path.display()),
        }
    }

    /// For the status bar: the language server's state and the file's problems.
    pub fn status(&self) -> Option<String> {
        if let Some(e) = self.lsp.config_error() {
            return Some(e.to_owned());
        }
        let doc = self.active_document()?;
        let status = self.lsp.status(&doc.path)?;
        let diagnostics = self.lsp.diagnostics(&doc.path);
        let count = |s| diagnostics.iter().filter(|d| d.severity == s).count();
        let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
        Some(match (errors, warnings) {
            (0, 0) => status,
            _ => format!("{status}   ⛔ {errors}  ⚠ {warnings}"),
        })
    }

    fn close(&mut self, index: usize) {
        let doc = self.documents.remove(index);
        self.lsp.close(&doc.path);
        self.completion = None;
        self.active = match self.active {
            _ if self.documents.is_empty() => None,
            Some(active) if active > index || active == self.documents.len() => Some(active - 1),
            other => other,
        };
    }

    /// Answers from the language servers.
    fn handle_lsp(&mut self) {
        for response in self.lsp.poll() {
            match response {
                lsp::Response::Hover { id, text } => {
                    if let Some((request, word)) = self.hover.request
                        && request == id
                    {
                        self.hover.request = None;
                        self.hover.text = text.map(|t| (word, t));
                    }
                }
                lsp::Response::Completion { id, items } => {
                    let Some(request) = &mut self.completion_request else {
                        continue;
                    };
                    if request.id != id {
                        continue;
                    }
                    if items.is_empty() && request.retries < COMPLETION_RETRIES {
                        request.retries += 1;
                        request.retry_at = Some(Instant::now() + Duration::from_millis(500));
                    } else {
                        let anchor = request.anchor;
                        self.completion_request = None;
                        self.completion =
                            (!items.is_empty()).then(|| Completion::new(anchor, items));
                    }
                }
                lsp::Response::Definition { id, locations } => {
                    if self.definition_request != Some(id) {
                        continue;
                    }
                    self.definition_request = None;
                    if let Some(location) = locations.into_iter().next() {
                        self.open(location.path);
                        if let Some(doc) = self.active.map(|i| &mut self.documents[i]) {
                            doc.jump_to =
                                Some(lsp::position_to_char_index(&doc.text, location.range.start));
                        }
                    }
                }
            }
        }

        // Retry an empty completion once its delay is over.
        if let (Some(request), Some(doc)) = (
            &mut self.completion_request,
            self.active.map(|i| &self.documents[i]),
        ) && request.retry_at.is_some_and(|at| Instant::now() >= at)
        {
            request.retry_at = None;
            let position = lsp::char_index_to_position(&doc.text, request.cursor);
            match self.lsp.completion(&doc.path, position) {
                Some(id) => request.id = id,
                None => self.completion_request = None,
            }
        }
    }

    fn request_completion(&mut self, index: usize) {
        let doc = &self.documents[index];
        let anchor = completion::word_start(&doc.text, doc.cursor);
        let position = lsp::char_index_to_position(&doc.text, doc.cursor);
        self.completion_request =
            self.lsp
                .completion(&doc.path, position)
                .map(|id| CompletionRequest {
                    id,
                    anchor,
                    cursor: doc.cursor,
                    retries: 0,
                    retry_at: None,
                });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.handle_lsp();
        if self
            .completion_request
            .as_ref()
            .is_some_and(|r| r.retry_at.is_some())
        {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        if self.documents.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.weak("Double-click a file in the Project view to open it")
            });
            return;
        }

        self.tab_bar(ui);
        ui.separator();

        let Some(index) = self.active else {
            return;
        };
        let ctx = ui.ctx().clone();
        let id = code_view::text_edit_id(&self.documents[index].path);
        let focused = ctx.memory(|m| m.has_focus(id));

        // Keys for the completion popup, taken before the text sees them.
        if focused && self.completion.is_some() {
            let key = ui.input_mut(|i| {
                [
                    (egui::Key::ArrowUp, Key::Up),
                    (egui::Key::ArrowDown, Key::Down),
                    (egui::Key::Enter, Key::Accept),
                    (egui::Key::Tab, Key::Accept),
                    (egui::Key::Escape, Key::Close),
                ]
                .into_iter()
                .find(|(key, _)| i.consume_key(egui::Modifiers::NONE, *key))
                .map(|(_, key)| key)
            });
            if let Some(key) = key {
                let closes = matches!(key, Key::Accept | Key::Close);
                let doc = &self.documents[index];
                let edit = self
                    .completion
                    .as_mut()
                    .and_then(|c| c.key(key, &doc.text, doc.cursor));
                if closes {
                    self.completion = None;
                }
                if let Some(edit) = edit {
                    self.apply_edit(index, edit);
                }
            }
        }

        // Ctrl+Space: complete. F12: go to definition.
        if focused {
            let (complete, definition) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Space),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::F12),
                )
            });
            if complete {
                self.request_completion(index);
            }
            if definition {
                let doc = &self.documents[index];
                self.go_to_definition(index, doc.cursor);
            }
        }

        let doc = &mut self.documents[index];
        let jump = doc.jump_to.take();
        if let Some(target) = jump {
            code_view::set_cursor(&ctx, id, target);
            doc.cursor = target;
        }
        let diagnostics = self.lsp.diagnostics(&doc.path).to_vec();
        let view = egui::ScrollArea::both()
            .auto_shrink(false)
            .id_salt(&doc.path)
            .show(ui, |ui| {
                let view = code_view::show(ui, id, &mut doc.text, doc.cursor, &diagnostics);
                if jump.is_some() {
                    ui.scroll_to_rect(view.char_rect(doc.cursor), Some(egui::Align::Center));
                }
                view
            })
            .inner;

        if let Some(cursor) = view.cursor {
            doc.cursor = cursor;
        }
        if view.changed {
            doc.dirty = true;
            self.lsp.change(&doc.path, &doc.text);
            self.after_typing(index);
        }
        if let Some(target) = view.ctrl_clicked {
            self.go_to_definition(index, target);
        }
        self.hover_ui(&ctx, index, &view, &diagnostics);

        let doc = &self.documents[index];
        if let Some(completion) = &mut self.completion
            && let Some(edit) =
                completion.ui(&ctx, view.char_rect(doc.cursor), &doc.text, doc.cursor)
        {
            self.completion = None;
            self.apply_edit(index, edit);
        }
    }

    /// Opens completion after `.` / `::`, and keeps an open popup in step with the text.
    fn after_typing(&mut self, index: usize) {
        let doc = &self.documents[index];
        let before: Vec<char> = doc.text.chars().take(doc.cursor).collect();
        let last = before.last().copied();
        let trigger = last == Some('.') || before.ends_with(&[':', ':']);
        if trigger {
            self.completion = None;
            self.request_completion(index);
            return;
        }
        if let Some(completion) = &self.completion {
            let typed = completion::typed(&doc.text, completion.anchor, doc.cursor);
            let still_in_word =
                doc.cursor >= completion.anchor && typed.chars().all(completion::is_word_char);
            if !still_in_word || completion.is_empty(&typed) {
                self.completion = None;
            }
        }
    }

    fn apply_edit(&mut self, index: usize, edit: completion::Edit) {
        let doc = &mut self.documents[index];
        let byte = |char_index: usize| {
            doc.text
                .char_indices()
                .nth(char_index)
                .map_or(doc.text.len(), |(b, _)| b)
        };
        let (start, end) = (byte(edit.start), byte(edit.end.max(edit.start)));
        doc.text.replace_range(start..end, &edit.text);
        doc.jump_to = Some(edit.start + edit.text.chars().count());
        doc.dirty = true;
        self.lsp.change(&doc.path, &doc.text);
    }

    fn go_to_definition(&mut self, index: usize, at: usize) {
        let doc = &self.documents[index];
        let position = lsp::char_index_to_position(&doc.text, at);
        self.definition_request = self.lsp.definition(&doc.path, position);
    }

    /// Tooltip with the diagnostics and the server's hover text for the word under the
    /// mouse, once it rested there a moment.
    fn hover_ui(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        view: &code_view::CodeView,
        diagnostics: &[lsp::Diagnostic],
    ) {
        let doc = &self.documents[index];
        let Some(hovered) = view.hovered.filter(|_| self.completion.is_none()) else {
            self.hover.resting = None;
            return;
        };
        let word = completion::word_start(&doc.text, hovered + 1);
        let now = Instant::now();
        match self.hover.resting {
            Some((resting, _)) if resting == word => {}
            _ => {
                self.hover.resting = Some((word, now));
                self.hover.text = None;
                self.hover.request = None;
            }
        }
        let Some((_, since)) = self.hover.resting else {
            return;
        };
        let rested = now - since >= HOVER_DELAY;
        if !rested {
            ctx.request_repaint_after(HOVER_DELAY);
            return;
        }
        if self.hover.request.is_none() && self.hover.text.is_none() {
            let position = lsp::char_index_to_position(&doc.text, hovered);
            self.hover.request = self.lsp.hover(&doc.path, position).map(|id| (id, word));
        }

        let messages: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                let start = lsp::position_to_char_index(&doc.text, d.range.start);
                let end = lsp::position_to_char_index(&doc.text, d.range.end).max(start + 1);
                (start..end).contains(&hovered)
            })
            .collect();
        let text = self
            .hover
            .text
            .as_ref()
            .filter(|(w, _)| *w == word)
            .map(|(_, t)| t);
        if messages.is_empty() && text.is_none() {
            return;
        }
        let Some(pointer) = ctx.pointer_hover_pos() else {
            return;
        };
        egui::Tooltip::always_open(
            ctx.clone(),
            view.response.layer_id,
            view.response.id,
            pointer,
        )
        .show(|ui| {
            ui.set_max_width(520.0);
            for diagnostic in &messages {
                let color = code_view::severity_color(diagnostic.severity, ui.visuals().dark_mode);
                let source = diagnostic
                    .source
                    .as_deref()
                    .map(|s| format!("{s}: "))
                    .unwrap_or_default();
                ui.colored_label(color, format!("{source}{}", diagnostic.message));
            }
            if let Some(text) = text {
                if !messages.is_empty() {
                    ui.separator();
                }
                ui.label(egui::RichText::new(text).monospace());
            }
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        egui::ScrollArea::horizontal()
            .id_salt("editor_tabs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, doc) in self.documents.iter().enumerate() {
                        let tab = ui
                            .selectable_label(self.active == Some(index), doc.title())
                            .on_hover_text(doc.path.display().to_string());
                        if tab.clicked() {
                            self.active = Some(index);
                            self.completion = None;
                        }
                        if tab.middle_clicked() || ui.small_button("🗙").clicked() {
                            to_close = Some(index);
                        }
                        ui.separator();
                    }
                });
            });
        if let Some(index) = to_close {
            self.close(index);
        }
    }
}

/// Paths from the project tree and from language servers can differ in drive-letter case.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
}
