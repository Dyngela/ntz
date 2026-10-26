//! Conversions between char indices in the editor's text and LSP positions (lines and
//! UTF-16 code units).

use super::Position;

pub fn char_index_to_position(text: &str, char_index: usize) -> Position {
    let mut line = 0;
    let mut character = 0;
    for c in text.chars().take(char_index) {
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    Position { line, character }
}

/// Clamps to the end of the line (or of the text) when the position is past it.
pub fn position_to_char_index(text: &str, position: Position) -> usize {
    let mut index = 0;
    let mut line = 0;
    let mut chars = text.chars().peekable();
    while line < position.line {
        match chars.next() {
            Some('\n') => line += 1,
            Some(_) => {}
            None => return index,
        }
        index += 1;
    }
    let mut units = 0;
    while let Some(&c) = chars.peek() {
        if c == '\n' || c == '\r' || units >= position.character {
            break;
        }
        units += c.len_utf16() as u32;
        index += 1;
        chars.next();
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_round_trip() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        assert_eq!(char_index_to_position(text, 16), pos(1, 4));
        assert_eq!(position_to_char_index(text, pos(1, 4)), 16);
    }

    #[test]
    fn utf16_units_for_non_bmp_chars() {
        // "🦀" is one char but two UTF-16 code units.
        let text = "a🦀b";
        assert_eq!(char_index_to_position(text, 2), pos(0, 3));
        assert_eq!(position_to_char_index(text, pos(0, 3)), 2);
    }

    #[test]
    fn past_the_end_clamps() {
        let text = "ab\r\ncd";
        assert_eq!(position_to_char_index(text, pos(0, 99)), 2);
        assert_eq!(position_to_char_index(text, pos(9, 0)), 6);
    }
}
