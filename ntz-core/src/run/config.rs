use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    pub name: String,
    /// Command line, run by the system shell (`cmd /C` on Windows, `sh -c` elsewhere), so
    /// `&&`, pipes and `.cmd` tools such as `npm` work as in a terminal.
    pub command: String,
    /// Relative to the project root (or absolute); the root itself when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
}

impl RunConfig {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            working_dir: String::new(),
        }
    }

    pub fn resolve_working_dir(&self, project_root: &Path) -> PathBuf {
        match self.working_dir.trim() {
            "" => project_root.to_path_buf(),
            dir => project_root.join(dir),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct RunFile {
    #[serde(default)]
    configurations: Vec<RunConfig>,
}

/// Where a project's custom configurations are saved.
pub fn config_file(project_root: &Path) -> PathBuf {
    project_root.join(".ntz").join("run.json")
}

/// The project's custom configurations; none if the file doesn't exist yet.
pub fn load(project_root: &Path) -> Result<Vec<RunConfig>, String> {
    let path = config_file(project_root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    let file: RunFile =
        serde_json::from_str(&text).map_err(|e| format!("Invalid {}: {e}", path.display()))?;
    Ok(file.configurations)
}

pub fn save(project_root: &Path, configurations: &[RunConfig]) -> Result<(), String> {
    let path = config_file(project_root);
    let file = RunFile {
        configurations: configurations.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    fs::write(&path, json + "\n").map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let root = std::env::temp_dir().join(format!("ntz-run-config-{}", std::process::id()));
        let configs = vec![
            RunConfig::new("app", "cargo run"),
            RunConfig {
                working_dir: "web".into(),
                ..RunConfig::new("front", "npm start")
            },
        ];
        save(&root, &configs).unwrap();
        assert_eq!(load(&root).unwrap(), configs);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_file_means_no_configurations() {
        let root = std::env::temp_dir().join("ntz-run-config-missing");
        assert_eq!(load(&root).unwrap(), Vec::new());
    }
}
