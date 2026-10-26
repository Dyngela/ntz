//! Configurations derived from the project's build files.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use super::RunConfig;
use super::process::hide_console;

/// Configurations for the project at `project_root`: one `cargo run` per binary of a Cargo
/// workspace plus `cargo test`, and one `npm run` per `package.json` script. Blocking (it
/// runs `cargo metadata`); call it off the UI thread.
pub fn detect(project_root: &Path) -> Vec<RunConfig> {
    let mut configs = cargo(project_root);
    configs.extend(npm(project_root));
    configs
}

fn cargo(root: &Path) -> Vec<RunConfig> {
    if !root.join("Cargo.toml").is_file() {
        return Vec::new();
    }
    let mut command = Command::new("cargo");
    command
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_slice::<Value>(&output.stdout) else {
        return Vec::new();
    };
    let mut configs = cargo_binaries(&metadata);
    configs.push(RunConfig::new("Test workspace", "cargo test --workspace"));
    configs
}

/// `cargo run` configurations for every binary target in `cargo metadata` output.
fn cargo_binaries(metadata: &Value) -> Vec<RunConfig> {
    let packages = metadata["packages"].as_array().into_iter().flatten();
    let mut configs = Vec::new();
    for package in packages {
        let Some(package_name) = package["name"].as_str() else {
            continue;
        };
        for target in package["targets"].as_array().into_iter().flatten() {
            let is_bin = target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|k| k == "bin"));
            if let (true, Some(bin)) = (is_bin, target["name"].as_str()) {
                configs.push(RunConfig::new(
                    bin,
                    format!("cargo run -p {package_name} --bin {bin}"),
                ));
            }
        }
    }
    configs
}

fn npm(root: &Path) -> Vec<RunConfig> {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(package) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    package["scripts"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(script, _)| RunConfig::new(format!("npm: {script}"), format!("npm run {script}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_config_per_binary_target() {
        let metadata = serde_json::json!({
            "packages": [
                { "name": "ntz-entrypoint", "targets": [{ "name": "ntz", "kind": ["bin"] }] },
                { "name": "ntz-core", "targets": [{ "name": "ntz_core", "kind": ["lib"] }] },
                { "name": "tools", "targets": [
                    { "name": "gen", "kind": ["bin"] },
                    { "name": "bench", "kind": ["bench"] }
                ] }
            ]
        });
        assert_eq!(
            cargo_binaries(&metadata),
            vec![
                RunConfig::new("ntz", "cargo run -p ntz-entrypoint --bin ntz"),
                RunConfig::new("gen", "cargo run -p tools --bin gen"),
            ]
        );
    }

    #[test]
    fn detects_this_workspace() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let names: Vec<_> = detect(root).into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"ntz".to_owned()), "{names:?}");
        assert!(names.contains(&"Test workspace".to_owned()), "{names:?}");
    }
}
