//! Which language server handles which files. Built-in defaults, overridable per project in
//! `.ntz/lsp.json`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Shown in the status bar; a project entry with a default's name replaces it.
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// File extensions it handles, without the dot.
    pub extensions: Vec<String>,
}

impl ServerConfig {
    fn new(name: &str, command: &str, args: &[&str], extensions: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            command: command.to_owned(),
            args: args.iter().map(|&a| a.to_owned()).collect(),
            extensions: extensions.iter().map(|&e| e.to_owned()).collect(),
        }
    }

    fn defaults() -> Vec<Self> {
        vec![
            Self::new("rust-analyzer", "rust-analyzer", &[], &["rs"]),
            Self::new("gopls", "gopls", &[], &["go"]),
            Self::new("pyright", "pyright-langserver", &["--stdio"], &["py"]),
            Self::new(
                "typescript-language-server",
                "typescript-language-server",
                &["--stdio"],
                &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
            ),
            Self::new("zls", "zls", &[], &["zig"]),
            Self::new(
                "clangd",
                "clangd",
                &[],
                &["c", "h", "cc", "cpp", "cxx", "hpp", "hh"],
            ),
        ]
    }

    pub fn handles(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
}

/// LSP's identifier for a file's language.
pub fn language_id(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "zig" => "zig",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        _ => "plaintext",
    }
}

#[derive(Default, Serialize, Deserialize)]
struct LspFile {
    #[serde(default)]
    servers: Vec<ServerConfig>,
}

/// Where a project overrides or adds servers.
pub fn config_file(project_root: &Path) -> PathBuf {
    project_root.join(".ntz").join("lsp.json")
}

/// The defaults, with the project's `.ntz/lsp.json` applied on top.
pub fn load(project_root: &Path) -> (Vec<ServerConfig>, Option<String>) {
    let mut servers = ServerConfig::defaults();
    let path = config_file(project_root);
    let file = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<LspFile>(&text)
            .map_err(|e| format!("Invalid {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LspFile::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    };
    match file {
        Ok(file) => {
            for custom in file.servers {
                servers.retain(|s| s.name != custom.name);
                // Project servers win when both claim an extension.
                servers.insert(0, custom);
            }
            (servers, None)
        }
        Err(e) => (servers, Some(e)),
    }
}

/// Finds `program` like a shell would, including `.cmd`/`.bat` launchers (npm installs
/// servers that way), which `Command::new("name")` alone doesn't find on Windows.
pub fn resolve_program(program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.to_path_buf();
    }
    let extensions: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
        pathext.split(';').map(|e| e.to_ascii_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    let dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    for dir in dirs.into_iter().flatten() {
        for ext in &extensions {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_entries_replace_defaults_by_name() {
        let root = std::env::temp_dir().join(format!("ntz-lsp-config-{}", std::process::id()));
        fs::create_dir_all(root.join(".ntz")).unwrap();
        fs::write(
            config_file(&root),
            r#"{ "servers": [ { "name": "zls", "command": "C:/tools/zls.exe", "extensions": ["zig"] } ] }"#,
        )
        .unwrap();
        let (servers, error) = load(&root);
        assert_eq!(error, None);
        let zls: Vec<_> = servers.iter().filter(|s| s.name == "zls").collect();
        assert_eq!(zls.len(), 1);
        assert_eq!(zls[0].command, "C:/tools/zls.exe");
        assert!(servers.iter().any(|s| s.name == "rust-analyzer"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn extensions_pick_the_server() {
        let (servers, _) = load(Path::new("does-not-exist"));
        let server = servers.iter().find(|s| s.handles(Path::new("src/Main.RS")));
        assert_eq!(server.map(|s| s.name.as_str()), Some("rust-analyzer"));
        assert_eq!(language_id(Path::new("app.tsx")), "typescriptreact");
    }
}
