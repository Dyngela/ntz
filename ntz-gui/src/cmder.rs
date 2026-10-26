//! Locating and installing Cmder, the shell embedded in the Terminal tool window.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// Cmder mini: ConEmu + clink, without the bundled Git for Windows.
pub const DOWNLOAD_URL: &str =
    "https://github.com/cmderdev/cmder/releases/latest/download/cmder_mini.zip";

/// How deep under the system drive to look for a `cmder` folder (e.g. `C:\dev\tools\cmder`).
const SCAN_DEPTH: usize = 3;

pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    Extracting { done: usize, total: usize },
}

/// Where ntz installs Cmder when the user agrees: `%LOCALAPPDATA%\ntz\cmder`.
pub fn managed_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("ntz").join("cmder"))
}

pub fn conemu_exe(cmder_root: &Path) -> Option<PathBuf> {
    let dir = cmder_root.join("vendor").join("conemu-maximus5");
    ["ConEmu64.exe", "ConEmu.exe"]
        .into_iter()
        .map(|exe| dir.join(exe))
        .find(|path| path.is_file())
}

pub fn is_cmder(root: &Path) -> bool {
    conemu_exe(root).is_some() && root.join("vendor").join("init.bat").is_file()
}

/// File where ntz remembers the Cmder it uses, so a custom location survives restarts
/// and the drive scan only runs once.
fn remembered_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("ntz").join("cmder_root.txt"))
}

pub fn remember(root: &Path) {
    if let Some(file) = remembered_file() {
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(file, root.as_os_str().as_encoded_bytes());
    }
}

fn remembered() -> Option<PathBuf> {
    let root = fs::read_to_string(remembered_file()?).ok()?;
    Some(PathBuf::from(root.trim()))
}

/// Finds a Cmder install: `CMDER_ROOT`, then the remembered location, then `PATH`, then usual
/// install locations, then a shallow scan of the system drive. Blocking; run it off the UI thread.
pub fn locate() -> Option<PathBuf> {
    let env_root = std::env::var_os("CMDER_ROOT").map(PathBuf::from);
    let on_path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let on_path = on_path
        .into_iter()
        .filter(|dir| dir.join("Cmder.exe").is_file());

    let env_dir = |var: &str, rest: &[&str]| {
        std::env::var_os(var).map(|dir| rest.iter().fold(PathBuf::from(dir), |p, s| p.join(s)))
    };
    let system_root = system_root();
    let known = [
        managed_dir(),
        env_dir("USERPROFILE", &["scoop", "apps", "cmder", "current"]),
        env_dir("USERPROFILE", &["scoop", "apps", "cmder-full", "current"]),
        env_dir("LOCALAPPDATA", &["cmder"]),
        env_dir("ProgramFiles", &["cmder"]),
        Some(system_root.join("tools").join("cmder")),
        Some(system_root.join("cmder")),
    ];

    env_root
        .into_iter()
        .chain(remembered())
        .chain(on_path)
        .chain(known.into_iter().flatten())
        .find(|root| is_cmder(root))
        .or_else(|| scan(system_root, SCAN_DEPTH))
}

/// Root of the system drive, e.g. `C:\`.
fn system_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    PathBuf::from(format!("{drive}\\"))
}

/// Breadth-first search for a directory named `cmder` that holds a Cmder install.
fn scan(root: PathBuf, max_depth: usize) -> Option<PathBuf> {
    let mut level = vec![root];
    for _ in 0..max_depth {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                // Junctions report as symlinks, so they're skipped along with plain files.
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.starts_with(['$', '.'])
                    || ["windows", "node_modules"].contains(&name.as_str())
                {
                    continue;
                }
                if name == "cmder" && is_cmder(&entry.path()) {
                    return Some(entry.path());
                }
                next.push(entry.path());
            }
        }
        level = next;
    }
    None
}

/// Checks that Cmder can be installed in `dest`: an absolute path to a new or empty folder.
/// Never installing over existing content means a mistyped path can't destroy anything.
pub fn check_install_dir(dest: &Path) -> Result<(), String> {
    if !dest.is_absolute() {
        return Err("Enter a full path, e.g. C:\\tools\\cmder".into());
    }
    if dest.is_file() {
        return Err(format!("{} is a file", dest.display()));
    }
    let not_empty = fs::read_dir(dest).is_ok_and(|mut entries| entries.next().is_some());
    if not_empty {
        return Err(format!(
            "{} isn't empty: choose a new or empty folder",
            dest.display()
        ));
    }
    Ok(())
}

/// Downloads Cmder mini and extracts it to `dest`. Blocking; run it off the UI thread.
pub fn install(dest: &Path, mut on_progress: impl FnMut(Progress)) -> Result<(), String> {
    check_install_dir(dest)?;
    let archive = download(&mut on_progress)?;

    // Extract next to `dest` and rename at the end, so a half-extracted install is never found.
    let mut staging = dest.as_os_str().to_owned();
    staging.push(".partial");
    let staging = PathBuf::from(staging);
    let _ = fs::remove_dir_all(&staging);
    extract(archive, &staging, &mut on_progress).map_err(|e| format!("Extraction failed: {e}"))?;
    if !is_cmder(&staging) {
        return Err("The downloaded archive doesn't look like Cmder".into());
    }
    // `dest` is at most an empty folder (checked above); `remove_dir` refuses anything else.
    if dest.exists() {
        fs::remove_dir(dest).map_err(|e| format!("Cannot use {}: {e}", dest.display()))?;
    }
    fs::rename(&staging, dest).map_err(|e| format!("Cannot move Cmder into place: {e}"))
}

fn download(on_progress: &mut impl FnMut(Progress)) -> Result<Vec<u8>, String> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

    // Windows' own TLS stack and certificate store, so corporate proxies' CAs are trusted.
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .build()
        .new_agent();

    let failed = |e: &dyn std::fmt::Display| format!("Download failed: {e}");
    let mut response = agent.get(DOWNLOAD_URL).call().map_err(|e| failed(&e))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok()?.parse().ok());

    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| failed(&e))?;
        if n == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..n]);
        on_progress(Progress::Downloading {
            done: bytes.len() as u64,
            total,
        });
    }
}

fn extract(
    archive: Vec<u8>,
    dest: &Path,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive))?;
    let total = archive.len();
    for index in 0..total {
        let mut file = archive.by_index(index)?;
        // `enclosed_name` rejects absolute paths and `..` components.
        let Some(relative) = file.enclosed_name() else {
            continue;
        };
        let out = dest.join(relative);
        if file.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut file, &mut fs::File::create(&out)?)?;
        }
        on_progress(Progress::Extracting {
            done: index + 1,
            total,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "downloads Cmder from GitHub; set NTZ_CMDER_TEST_DIR to a scratch folder"]
    fn install_downloads_a_working_cmder() {
        let dir =
            PathBuf::from(std::env::var_os("NTZ_CMDER_TEST_DIR").expect("NTZ_CMDER_TEST_DIR"));
        let dest = dir.join("cmder");
        install(&dest, |_| {}).unwrap();
        assert!(is_cmder(&dest));
    }
}
