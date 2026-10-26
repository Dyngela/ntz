//! A run's output: ANSI colors, stderr in red, the user's input echoed, the status line.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, FontId, TextFormat, text::LayoutJob};
use ntz_core::run::ansi::{self, Color, Span, Style};
use ntz_core::run::{OutputLine, Stream};

/// Older lines are dropped past this, so a chatty program can't eat all the memory.
const MAX_LINES: usize = 20_000;

/// Style of the user's input, echoed in the output like a console would.
const INPUT_STYLE: Style = Style {
    fg: Some(Color::Green),
    bold: false,
};

#[derive(Default)]
pub struct Output {
    lines: VecDeque<Line>,
}

/// One line, parsed once when it arrives.
struct Line {
    stream: Stream,
    spans: Vec<Span>,
    /// Whether the program colored the line itself.
    colored: bool,
    /// Still being written (e.g. a prompt); replaced by the next line from its stream.
    partial: bool,
}

impl Output {
    pub fn push(&mut self, line: OutputLine) {
        let spans = ansi::parse(&line.text);
        let line = Line {
            stream: line.stream,
            colored: spans.iter().any(|s| s.style.fg.is_some()),
            spans,
            partial: line.partial,
        };
        if let Some(last) = self.lines.back_mut()
            && last.partial
            && last.stream == line.stream
        {
            *last = line;
            return;
        }
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    /// Echoes what the user sent. After a prompt (a partial line) it goes on the prompt's
    /// line, which is final from then on.
    pub fn push_input(&mut self, text: &str) {
        let span = Span {
            text: text.to_owned(),
            style: INPUT_STYLE,
        };
        match self.lines.back_mut() {
            Some(last) if last.partial => {
                last.spans.push(span);
                last.partial = false;
            }
            _ => self.lines.push_back(Line {
                stream: Stream::Stdout,
                spans: vec![span],
                colored: true,
                partial: false,
            }),
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

impl Line {
    fn layout(&self, ui: &egui::Ui) -> LayoutJob {
        let visuals = ui.visuals();
        // Uncolored stderr is shown in red, like JetBrains IDEs. Lines the program colored
        // itself (cargo's "Compiling…", warnings) keep their colors.
        let default = if self.stream == Stream::Stderr && !self.colored {
            visuals.error_fg_color
        } else {
            visuals.text_color()
        };
        let mut job = LayoutJob::default();
        for span in &self.spans {
            let color = span
                .style
                .fg
                .map_or(default, |c| ansi_color(c, visuals.dark_mode));
            let font = FontId::monospace(13.0);
            let mut format = TextFormat::simple(font, color);
            if span.style.bold {
                // egui's monospace font has no bold face; brighten instead.
                format.color = color.gamma_multiply(1.15);
            }
            job.append(&span.text, 0.0, format);
        }
        job
    }
}

pub fn show(ui: &mut egui::Ui, output: &Output, status: &str, tab: usize) {
    let lines = &output.lines;
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    let rows = lines.len() + usize::from(!status.is_empty());
    egui::ScrollArea::both()
        .id_salt(("run_output", tab))
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show_rows(ui, row_height, rows, |ui, range| {
            for row in range {
                match lines.get(row) {
                    Some(line) => {
                        ui.add(egui::Label::new(line.layout(ui)).extend());
                    }
                    None => {
                        ui.add(egui::Label::new(
                            egui::RichText::new(status).monospace().weak(),
                        ));
                    }
                }
            }
        });
}

fn ansi_color(color: Color, dark_mode: bool) -> Color32 {
    let (dark, light) = match color {
        Color::Black | Color::BrightBlack => (
            Color32::from_rgb(140, 140, 140),
            Color32::from_rgb(90, 90, 90),
        ),
        Color::Red | Color::BrightRed => (
            Color32::from_rgb(240, 100, 100),
            Color32::from_rgb(190, 30, 30),
        ),
        Color::Green | Color::BrightGreen => (
            Color32::from_rgb(110, 200, 110),
            Color32::from_rgb(30, 130, 40),
        ),
        Color::Yellow | Color::BrightYellow => (
            Color32::from_rgb(230, 200, 90),
            Color32::from_rgb(150, 110, 0),
        ),
        Color::Blue | Color::BrightBlue => (
            Color32::from_rgb(110, 160, 250),
            Color32::from_rgb(30, 80, 200),
        ),
        Color::Magenta | Color::BrightMagenta => (
            Color32::from_rgb(210, 130, 230),
            Color32::from_rgb(140, 40, 160),
        ),
        Color::Cyan | Color::BrightCyan => (
            Color32::from_rgb(90, 200, 210),
            Color32::from_rgb(0, 120, 140),
        ),
        Color::White | Color::BrightWhite => (
            Color32::from_rgb(230, 230, 230),
            Color32::from_rgb(60, 60, 60),
        ),
    };
    if dark_mode { dark } else { light }
}
