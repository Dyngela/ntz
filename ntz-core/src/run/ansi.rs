//! Minimal ANSI escape handling for program output: SGR colors and bold are kept as spans,
//! every other escape sequence is dropped.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl Color {
    const NORMAL: [Color; 8] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::White,
    ];
    const BRIGHT: [Color; 8] = [
        Color::BrightBlack,
        Color::BrightRed,
        Color::BrightGreen,
        Color::BrightYellow,
        Color::BrightBlue,
        Color::BrightMagenta,
        Color::BrightCyan,
        Color::BrightWhite,
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// Splits `line` into styled spans.
pub fn parse(line: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\x1b' {
            text.push(c);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue; // Lone ESC or a non-CSI sequence: drop the ESC.
        }
        chars.next();
        let mut params = String::new();
        let mut terminator = None;
        for c in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&c) {
                terminator = Some(c);
                break;
            }
            params.push(c);
        }
        if terminator != Some('m') {
            continue; // Cursor movement, erase line, …: meaningless in a log, dropped.
        }
        let next = apply_sgr(style, &params);
        if next != style {
            if !text.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut text),
                    style,
                });
            }
            style = next;
        }
    }
    if !text.is_empty() {
        spans.push(Span { text, style });
    }
    spans
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u32> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mut codes = codes.into_iter();
    while let Some(code) = codes.next() {
        match code {
            0 => style = Style::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            30..=37 => style.fg = Some(Color::NORMAL[(code - 30) as usize]),
            90..=97 => style.fg = Some(Color::BRIGHT[(code - 90) as usize]),
            39 => style.fg = None,
            // 256-color / true-color foreground: skip their arguments, keep the default color.
            38 => match codes.next() {
                Some(5) => {
                    codes.next();
                }
                Some(2) => {
                    codes.nth(2);
                }
                _ => {}
            },
            _ => {}
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(
            parse("hello"),
            vec![Span {
                text: "hello".into(),
                style: Style::default()
            }]
        );
    }

    #[test]
    fn cargo_style_line() {
        let spans = parse("\x1b[1m\x1b[32m   Compiling\x1b[0m ntz v0.1.0");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "   Compiling");
        assert_eq!(
            spans[0].style,
            Style {
                fg: Some(Color::Green),
                bold: true
            }
        );
        assert_eq!(spans[1].text, " ntz v0.1.0");
        assert_eq!(spans[1].style, Style::default());
    }

    #[test]
    fn non_color_sequences_are_dropped() {
        let spans = parse("\x1b[2Kprogress\x1b[1A done");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "progress done");
    }
}
