//! The text of one file: a gutter with relative line numbers and diagnostic markers, the
//! editable text, and diagnostics underlined in it.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontId, Galley, Pos2, Rect, Stroke, TextStyle, text::CCursor,
    text::CCursorRange, text_edit::TextEditState,
};
use ntz_core::lsp::{Diagnostic, Severity, position_to_char_index};

/// What happened in the view this frame.
pub struct CodeView {
    pub response: egui::Response,
    pub changed: bool,
    /// Char index of the cursor, while the text has focus.
    pub cursor: Option<usize>,
    /// Char index under the mouse pointer.
    pub hovered: Option<usize>,
    /// Char index Ctrl+clicked (go to definition).
    pub ctrl_clicked: Option<usize>,
    galley: Arc<Galley>,
    galley_pos: Pos2,
}

impl CodeView {
    /// Screen rectangle of the character at `index` (zero width: the cursor before it).
    pub fn char_rect(&self, index: usize) -> Rect {
        self.galley
            .pos_from_cursor(CCursor::new(index))
            .translate(self.galley_pos.to_vec2())
    }
}

pub fn text_edit_id(path: &std::path::Path) -> egui::Id {
    egui::Id::new(("editor", path))
}

/// Puts the cursor at `index` and focuses the text, e.g. after go-to-definition.
pub fn set_cursor(ctx: &egui::Context, id: egui::Id, index: usize) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(index))));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

pub fn show(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    cursor: usize,
    diagnostics: &[Diagnostic],
) -> CodeView {
    let font = TextStyle::Monospace.resolve(ui.style());
    let line_count = text.lines().count().max(1);
    let digits = line_count.to_string().len().max(3);
    let digit_width = ui
        .painter()
        .layout_no_wrap("0".repeat(digits), font.clone(), Color32::WHITE)
        .size()
        .x;
    let gutter_width = digit_width + 22.0;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (gutter, _) = ui.allocate_exact_size(
            egui::vec2(gutter_width, ui.available_height()),
            egui::Sense::hover(),
        );
        let output = egui::TextEdit::multiline(text)
            .id(id)
            .code_editor()
            .desired_width(f32::INFINITY)
            // Fill the editor area even for a short file, so clicking anywhere below the
            // text still lands in it.
            .min_size(ui.available_size())
            .show(ui);

        let response = output.response.response;
        let galley = output.galley;
        let galley_pos = output.galley_pos;
        let cursor_now = output.cursor_range.map(|r| r.primary.index.0);
        let pointer_index = |pos: Pos2| galley.cursor_from_pos(pos - galley_pos).index.0;

        let hovered = response.hover_pos().map(pointer_index);
        let ctrl_clicked = (response.clicked() && ui.input(|i| i.modifiers.command))
            .then(|| response.interact_pointer_pos().map(pointer_index))
            .flatten();

        let view = CodeView {
            changed: response.changed(),
            response,
            cursor: cursor_now,
            hovered,
            ctrl_clicked,
            galley,
            galley_pos,
        };
        let cursor_line = line_of(text, cursor_now.unwrap_or(cursor));
        paint_gutter(ui, gutter, &view, cursor_line, diagnostics, &font);
        paint_squiggles(ui, &view, text, diagnostics);
        view
    })
    .inner
}

/// Zero-based line of the char at `index`.
fn line_of(text: &str, index: usize) -> usize {
    text.chars().take(index).filter(|&c| c == '\n').count()
}

/// Relative line numbers (the cursor's line shows its absolute number) and a dot on lines
/// with diagnostics.
fn paint_gutter(
    ui: &egui::Ui,
    gutter: Rect,
    view: &CodeView,
    cursor_line: usize,
    diagnostics: &[Diagnostic],
    font: &FontId,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let clip = ui.clip_rect();
    let worst_by_line = |line: usize| {
        diagnostics
            .iter()
            .filter(|d| d.range.start.line as usize == line)
            .map(|d| d.severity)
            .min()
    };

    let mut line = 0;
    let mut starts_line = true;
    for row in &view.galley.rows {
        let row_rect = row.rect().translate(view.galley_pos.to_vec2());
        if starts_line && row_rect.max.y >= clip.min.y && row_rect.min.y <= clip.max.y {
            let (number, color) = if line == cursor_line {
                (line + 1, visuals.strong_text_color())
            } else {
                (line.abs_diff(cursor_line), visuals.weak_text_color())
            };
            painter.text(
                Pos2::new(gutter.right() - 10.0, row_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                number.to_string(),
                font.clone(),
                color,
            );
            if let Some(severity) = worst_by_line(line) {
                painter.circle_filled(
                    Pos2::new(gutter.left() + 6.0, row_rect.center().y),
                    3.0,
                    severity_color(severity, visuals.dark_mode),
                );
            }
        }
        // A wrapped line continues on the next row without a number.
        starts_line = row.ends_with_newline;
        if row.ends_with_newline {
            line += 1;
        }
    }
}

/// A wavy underline under each diagnostic's range (to the end of its first row when it
/// spans several).
fn paint_squiggles(ui: &egui::Ui, view: &CodeView, text: &str, diagnostics: &[Diagnostic]) {
    let painter = ui.painter();
    for diagnostic in diagnostics {
        let start = position_to_char_index(text, diagnostic.range.start);
        let end = position_to_char_index(text, diagnostic.range.end);
        let start_rect = view.char_rect(start);
        let end_rect = view.char_rect(end);
        let right = if (end_rect.min.y - start_rect.min.y).abs() < 1.0 {
            end_rect.min.x
        } else {
            view.galley
                .rows
                .iter()
                .map(|r| r.rect().translate(view.galley_pos.to_vec2()))
                .find(|r| r.contains(start_rect.center()))
                .map_or(start_rect.min.x, |r| r.max.x)
        };
        // Empty ranges (e.g. "expected expression" at a point) still get a short mark.
        let right = right.max(start_rect.min.x + 6.0);
        let color = severity_color(diagnostic.severity, ui.visuals().dark_mode);
        wavy_line(
            painter,
            start_rect.min.x,
            right,
            start_rect.max.y - 1.0,
            color,
        );
    }
}

fn wavy_line(painter: &egui::Painter, left: f32, right: f32, y: f32, color: Color32) {
    let (step, amplitude) = (2.0, 1.5);
    let mut points = Vec::new();
    let mut x = left;
    let mut up = true;
    while x <= right {
        points.push(Pos2::new(x, if up { y - amplitude } else { y + amplitude }));
        x += step;
        up = !up;
    }
    painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
}

pub fn severity_color(severity: Severity, dark_mode: bool) -> Color32 {
    match (severity, dark_mode) {
        (Severity::Error, true) => Color32::from_rgb(240, 90, 90),
        (Severity::Error, false) => Color32::from_rgb(210, 30, 30),
        (Severity::Warning, true) => Color32::from_rgb(230, 190, 70),
        (Severity::Warning, false) => Color32::from_rgb(190, 130, 0),
        (Severity::Information, _) => Color32::from_rgb(80, 150, 240),
        (Severity::Hint, _) => Color32::GRAY,
    }
}
