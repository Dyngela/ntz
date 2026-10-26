//! One terminal session: a ConEmu instance running a Cmder task, docked in its own host window.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use super::native;
use crate::cmder;

/// Shells offered by the "+" menu, as tasks of Cmder's ConEmu configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::enum_variant_names,
    reason = "PowerShell is the product's name"
)]
pub enum Shell {
    Cmder,
    PowerShell,
    Bash,
}

impl Shell {
    pub const ALL: [Shell; 3] = [Shell::Cmder, Shell::PowerShell, Shell::Bash];

    pub fn name(self) -> &'static str {
        match self {
            Shell::Cmder => "Cmder",
            Shell::PowerShell => "PowerShell",
            Shell::Bash => "Git Bash",
        }
    }

    fn task(self) -> &'static str {
        match self {
            Shell::Cmder => "{cmd::Cmder}",
            Shell::PowerShell => "{PowerShell::PowerShell}",
            Shell::Bash => "{bash::bash}",
        }
    }

    /// Git Bash only ships with full Cmder, not Cmder mini.
    pub fn is_available(self, cmder_root: &Path) -> bool {
        match self {
            Shell::Bash => cmder_root
                .join("vendor")
                .join("git-for-windows")
                .join("bin")
                .join("bash.exe")
                .is_file(),
            Shell::Cmder | Shell::PowerShell => true,
        }
    }
}

pub struct Session {
    pub title: String,
    pub host: native::HostWindow,
    conemu: Option<Child>,
}

impl Session {
    /// Starts ConEmu docked in a new hidden host window sized to `rect` (physical pixels).
    pub fn spawn(
        parent: native::Hwnd,
        cmder_root: &Path,
        cwd: &Path,
        shell: Shell,
        title: String,
        rect: [i32; 4],
    ) -> Result<Self, String> {
        let conemu = cmder::conemu_exe(cmder_root)
            .ok_or_else(|| format!("ConEmu not found under {}", cmder_root.display()))?;
        let host = native::HostWindow::create(parent, rect)
            .ok_or("Failed to create the terminal host window")?;

        let mut command = Command::new(conemu);
        command
            .env("CMDER_ROOT", cmder_root)
            .args(["-NoSingle", "-InsideWnd", &host.handle_arg()])
            .arg("-Dir")
            .arg(cwd);
        // Like Cmder.exe: on first launch, seed the user config (which defines the tasks)
        // from the shipped default.
        let config = cmder_root.join("config").join("user-ConEmu.xml");
        let default_config = cmder_root.join("vendor").join("ConEmu.xml.default");
        if !config.is_file() && default_config.is_file() {
            let _ = std::fs::copy(&default_config, &config);
        }
        if config.is_file() {
            command.arg("-LoadCfgFile").arg(config);
        }
        // `-run` must come last: ConEmu treats the rest of the line as the command.
        command.args(["-run", shell.task()]);

        let conemu = command
            .spawn()
            .map_err(|e| format!("Failed to start ConEmu: {e}"))?;
        Ok(Self {
            title,
            host,
            conemu: Some(conemu),
        })
    }

    pub fn is_alive(&mut self) -> bool {
        self.conemu
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    /// Destroys the host, which makes ConEmu shut its consoles down cleanly, and hands back
    /// the process so the caller can wait for it.
    pub fn shut_down(&mut self) -> Option<Child> {
        self.host.destroy();
        self.conemu.take()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Closing a tab must not freeze the UI: wait for ConEmu off-thread.
        if let Some(conemu) = self.shut_down() {
            std::thread::spawn(move || wait_or_kill(vec![conemu], Duration::from_secs(2)));
        }
    }
}

/// Gives the processes `grace` to exit on their own, then kills the remaining ones.
pub fn wait_or_kill(mut processes: Vec<Child>, grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        processes.retain_mut(|p| matches!(p.try_wait(), Ok(None)));
        if processes.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for mut process in processes {
        let _ = process.kill();
    }
}
