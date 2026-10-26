//! `file://` URIs, as LSP identifies documents.

use std::path::{Path, PathBuf};

pub fn path_to_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/'); // `C:/x` → `file:///C:/x`
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(path.len());
    let mut iter = path.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    if cfg!(windows) {
        // `/C:/x` → `C:\x`
        let path = path.strip_prefix('/').unwrap_or(&path);
        Some(normalize(Path::new(&path.replace('/', "\\"))))
    } else {
        Some(PathBuf::from(path))
    }
}

/// One spelling per file, so paths from the editor and from servers compare equal: servers
/// may lowercase the drive letter.
pub fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if cfg!(windows) => {
            PathBuf::from(format!("{}{}", drive.to_ascii_uppercase(), &text[1..]))
        }
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_paths_round_trip() {
        let path = Path::new(r"C:\dev\my project\src\main.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///C:/dev/my%20project/src/main.rs");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    #[cfg(windows)]
    #[test]
    fn server_spellings_are_normalized() {
        assert_eq!(
            uri_to_path("file:///c%3A/dev/ntz/src/lib.rs").unwrap(),
            Path::new(r"C:\dev\ntz\src\lib.rs")
        );
    }
}
