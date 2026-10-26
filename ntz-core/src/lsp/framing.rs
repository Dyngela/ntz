//! LSP's base protocol: JSON messages preceded by a `Content-Length` header.

use std::io::{self, BufRead, Write};

use serde_json::Value;

pub fn write_message(out: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()
}

/// The next message, or `None` at end of stream.
pub fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut header = String::new();
    loop {
        header.clear();
        if input.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        let line = header.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue; // Stray blank line before the headers.
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let message = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "été" });
        let mut wire = Vec::new();
        write_message(&mut wire, &message).unwrap();
        write_message(&mut wire, &message).unwrap();
        let mut input = io::BufReader::new(wire.as_slice());
        assert_eq!(read_message(&mut input).unwrap(), Some(message.clone()));
        assert_eq!(read_message(&mut input).unwrap(), Some(message));
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    #[test]
    fn extra_headers_are_ignored() {
        let wire = b"Content-Type: application/vscode-jsonrpc\r\nContent-Length: 2\r\n\r\n{}";
        let mut input = io::BufReader::new(&wire[..]);
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some(serde_json::json!({}))
        );
    }
}
