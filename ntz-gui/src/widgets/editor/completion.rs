//! The completion popup: the server's items, narrowed down as the user keeps typing.

use eframe::egui;
use ntz_core::lsp::{CompletionItem, position_to_char_index};

pub struct Completion {
    /// Char index where the word being completed starts.
    pub anchor: usize,
    items: Vec<CompletionItem>,
    /// Index into the filtered items.
    selected: usize,
    /// Set when the selection moved with the keyboard, to scroll it into view.
    scroll_to_selected: bool,
}

pub enum Key {
    Up,
    Down,
    Accept,
    Close,
}

/// An edit to apply to the text: replace chars `start..end` with `text`.
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Completion {
    pub fn new(anchor: usize, items: Vec<CompletionItem>) -> Self {
        Self {
            anchor,
            items,
            selected: 0,
            scroll_to_selected: false,
        }
    }

    fn filtered(&self, typed: &str) -> Vec<&CompletionItem> {
        filter(&self.items, typed)
    }

    pub fn is_empty(&self, typed: &str) -> bool {
        self.filtered(typed).is_empty()
    }

    /// Handles a navigation key; returns the edit to apply if an item was accepted.
    pub fn key(&mut self, key: Key, text: &str, cursor: usize) -> Option<Edit> {
        let count = self.filtered(&typed(text, self.anchor, cursor)).len();
        match key {
            Key::Up => {
                self.selected = (self.selected + count - 1) % count.max(1);
                self.scroll_to_selected = true;
            }
            Key::Down => {
                self.selected = (self.selected + 1) % count.max(1);
                self.scroll_to_selected = true;
            }
            Key::Accept => return self.accept(self.selected, text, cursor),
            Key::Close => {}
        }
        None
    }

    fn accept(&self, index: usize, text: &str, cursor: usize) -> Option<Edit> {
        let typed = typed(text, self.anchor, cursor);
        let item = *self.filtered(&typed).get(index)?;
        let start = match item.replace {
            Some(range) => position_to_char_index(text, range.start).min(self.anchor),
            None => self.anchor,
        };
        Some(Edit {
            start,
            end: cursor,
            text: item.insert_text.clone(),
        })
    }

    /// Shows the popup under `cursor_rect`; returns the edit if an item was clicked.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        cursor_rect: egui::Rect,
        text: &str,
        cursor: usize,
    ) -> Option<Edit> {
        let typed = typed(text, self.anchor, cursor);
        let filtered = filter(&self.items, &typed);
        self.selected = self.selected.min(filtered.len().saturating_sub(1));
        let mut clicked = None;
        let scroll = std::mem::take(&mut self.scroll_to_selected);
        egui::Area::new(egui::Id::new("completion_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(cursor_rect.left_bottom() + egui::vec2(0.0, 2.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(260.0);
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for (index, item) in filtered.iter().enumerate().take(200) {
                                let label = match &item.detail {
                                    Some(detail) => {
                                        egui::RichText::new(format!("{}   {detail}", item.label))
                                    }
                                    None => egui::RichText::new(&item.label),
                                }
                                .monospace();
                                let response = ui.selectable_label(index == self.selected, label);
                                if index == self.selected && scroll {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                            }
                        });
                });
            });
        clicked.and_then(|index| self.accept(index, text, cursor))
    }
}

/// What the user typed of the word being completed.
pub fn typed(text: &str, anchor: usize, cursor: usize) -> String {
    text.chars()
        .skip(anchor)
        .take(cursor.saturating_sub(anchor))
        .collect()
}

/// Start of the identifier that ends at `cursor`.
pub fn word_start(text: &str, cursor: usize) -> usize {
    let before: Vec<char> = text.chars().take(cursor).collect();
    let mut start = before.len();
    while start > 0 && is_word_char(before[start - 1]) {
        start -= 1;
    }
    start
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Items matching what was typed since the anchor: prefix matches first, then the rest
/// that contain it.
fn filter<'a>(items: &'a [CompletionItem], typed: &str) -> Vec<&'a CompletionItem> {
    let typed = typed.to_lowercase();
    let (mut prefix, mut contains): (Vec<_>, Vec<_>) = (Vec::new(), Vec::new());
    for item in items {
        let text = item.filter_text.to_lowercase();
        if text.starts_with(&typed) {
            prefix.push(item);
        } else if text.contains(&typed) {
            contains.push(item);
        }
    }
    prefix.extend(contains);
    prefix
}
