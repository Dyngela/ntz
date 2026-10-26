use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use super::RunConfig;
use super::job::Job;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: Stream,
    /// The line without its line ending; may contain ANSI escape sequences.
    pub text: String,
    /// The line isn't finished yet (e.g. a prompt waiting for input). The next line from
    /// the same stream replaces it, until one arrives with `partial: false`.
    pub partial: bool,
}

/// A started configuration. Dropping it kills the whole process tree.
pub struct RunningProcess {
    child: Child,
    /// `None` if the process couldn't be put in a job (stop then only kills the child).
    job: Option<Job>,
    output: Receiver<OutputLine>,
    /// Lines for the program's stdin; `None` once input was closed.
    input: Option<Sender<String>>,
    /// Tells the readers that the partial line they hold was completed by input.
    input_sent: [Arc<AtomicBool>; 2],
}

impl RunningProcess {
    /// Starts `config` from the project at `project_root`. `notify` is called from
    /// background threads whenever output arrives (e.g. to request a repaint).
    pub fn spawn(
        config: &RunConfig,
        project_root: &Path,
        notify: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let mut command = shell_command(&config.command);
        command
            .current_dir(config.resolve_working_dir(project_root))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Output goes to a pipe, so tools would otherwise drop their colors…
            .env("CARGO_TERM_COLOR", "always")
            // …and Python would buffer it all and write it in the Windows code page.
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONIOENCODING", "utf-8");
        hide_console(&mut command);
        let mut child = command.spawn()?;

        let job = Job::new().ok().filter(|job| job.assign(&child).is_ok());

        let (tx, rx) = mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
        let input_sent = [Arc::default(), Arc::default()];
        if let Some(stdout) = child.stdout.take() {
            let flag = Arc::clone(&input_sent[0]);
            forward_output(stdout, Stream::Stdout, tx.clone(), flag, notify.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            let flag = Arc::clone(&input_sent[1]);
            forward_output(stderr, Stream::Stderr, tx, flag, notify);
        }
        let input = child.stdin.take().map(forward_input);
        Ok(Self {
            child,
            job,
            output: rx,
            input,
            input_sent,
        })
    }

    /// Output received since the last call.
    pub fn drain(&self) -> impl Iterator<Item = OutputLine> + '_ {
        self.output.try_iter()
    }

    /// Sends one line to the program's stdin, as if typed in a console. A partial line
    /// (a prompt) shown before it is final from now on.
    pub fn send_input(&self, line: &str) {
        if let Some(input) = &self.input {
            for flag in &self.input_sent {
                flag.store(true, Ordering::SeqCst);
            }
            let newline = if cfg!(windows) { "\r\n" } else { "\n" };
            let _ = input.send(format!("{line}{newline}"));
        }
    }

    /// Closes the program's stdin, which it sees as end of input (Ctrl+Z / Ctrl+D).
    pub fn close_input(&mut self) {
        self.input = None;
    }

    pub fn accepts_input(&self) -> bool {
        self.input.is_some()
    }

    /// `Some` once the process has exited.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Kills the process and everything it started.
    pub fn stop(&mut self) {
        match &self.job {
            Some(job) => job.terminate(),
            None => {
                let _ = self.child.kill();
            }
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Runs `command_line` through the system shell, verbatim.
fn shell_command(command_line: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd.exe");
        // `/S /C "…"`: cmd strips the outer quotes and runs the rest exactly as typed.
        command
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{command_line}\""));
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(command_line);
        command
    }
}

/// ntz is a GUI app: without this, every console program it starts opens a console window.
pub(crate) fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Writes lines to the program off the UI thread: a program that doesn't read its input
/// would otherwise block the UI once the pipe is full.
fn forward_input(mut stdin: impl Write + Send + 'static) -> Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in rx {
            if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                break;
            }
        }
        // Dropping `stdin` here closes the pipe: end of input for the program.
    });
    tx
}

/// Reads whatever the program writes, as it comes, so a prompt without a line ending shows
/// up right away (as a partial line) instead of waiting for the next newline.
fn forward_output(
    mut pipe: impl Read + Send + 'static,
    stream: Stream,
    tx: Sender<OutputLine>,
    input_sent: Arc<AtomicBool>,
    notify: Arc<dyn Fn() + Send + Sync>,
) {
    std::thread::spawn(move || {
        let send = |bytes: &[u8], partial: bool| {
            let text = line_text(bytes, partial);
            tx.send(OutputLine {
                stream,
                text,
                partial,
            })
            .is_ok()
        };
        let mut pending = Vec::new();
        let mut chunk = vec![0; 8192];
        loop {
            let n = match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            // The partial line was completed by the user's input, and is shown as such:
            // what comes next starts a new line.
            if input_sent.swap(false, Ordering::SeqCst) {
                pending.clear();
            }
            pending.extend_from_slice(&chunk[..n]);
            while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                if !send(&line, false) {
                    return;
                }
            }
            if !pending.is_empty() && !send(&pending, true) {
                return;
            }
            notify();
        }
        if !pending.is_empty() && !input_sent.load(Ordering::SeqCst) {
            send(&pending, false);
            notify();
        }
    });
}

/// Decodes one line and keeps what a console would show: the text after the last carriage
/// return (progress bars rewrite their line with `\r`).
fn line_text(bytes: &[u8], partial: bool) -> String {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let text = decode(&bytes[..end], partial);
    match text.rfind('\r') {
        Some(cr) => text[cr + 1..].to_owned(),
        None => text,
    }
}

/// UTF-8 when valid; otherwise the console's OEM code page (850 in Western Europe), which
/// Windows' own tools write in: cmd's messages, `ping`, `dir`… even after `chcp 65001`.
fn decode(bytes: &[u8], partial: bool) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        // A partial line can end in the middle of a UTF-8 character: show what's complete.
        Err(e) if partial && e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => decode_oem(bytes),
    }
}

#[cfg(windows)]
fn decode_oem(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{CP_OEMCP, MultiByteToWideChar};

    let Ok(len) = i32::try_from(bytes.len()) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    unsafe {
        let wide_len =
            MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
        if wide_len <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; wide_len as usize];
        MultiByteToWideChar(
            CP_OEMCP,
            0,
            bytes.as_ptr(),
            len,
            wide.as_mut_ptr(),
            wide_len,
        );
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(not(windows))]
fn decode_oem(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn spawn(command: &str) -> RunningProcess {
        let config = RunConfig::new("test", command);
        RunningProcess::spawn(&config, &std::env::temp_dir(), || {}).unwrap()
    }

    fn wait(process: &mut RunningProcess) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = process.try_wait() {
                break status;
            }
            assert!(Instant::now() < deadline, "process didn't finish");
            std::thread::sleep(Duration::from_millis(20));
        };
        // Let the reader threads flush what's left in the pipes.
        std::thread::sleep(Duration::from_millis(200));
        status
    }

    fn complete_lines(process: &RunningProcess) -> Vec<(Stream, String)> {
        process
            .drain()
            .filter(|l| !l.partial)
            .map(|l| (l.stream, l.text))
            .collect()
    }

    #[test]
    fn captures_both_streams_and_exit_code() {
        let mut process = spawn("echo out&& echo err 1>&2&& exit 3");
        let status = wait(&mut process);
        let lines = complete_lines(&process);
        assert!(lines.contains(&(Stream::Stdout, "out".into())), "{lines:?}");
        assert!(
            lines.contains(&(Stream::Stderr, "err ".into())),
            "{lines:?}"
        );
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn accents_survive() {
        let mut process = spawn("echo déjà vu");
        wait(&mut process);
        let lines = complete_lines(&process);
        assert!(
            lines.contains(&(Stream::Stdout, "déjà vu".into())),
            "{lines:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn prompt_shows_before_input_then_input_is_read() {
        // `set /p` prints its prompt without a line ending, then reads a line.
        let mut process = spawn("set /p NAME=Name: & call echo hello %NAME%");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let prompt = process.drain().any(|l| l.partial && l.text == "Name: ");
            if prompt {
                break;
            }
            assert!(Instant::now() < deadline, "no prompt");
            std::thread::sleep(Duration::from_millis(20));
        }
        process.send_input("Zoé");
        wait(&mut process);
        let lines = complete_lines(&process);
        assert_eq!(lines, vec![(Stream::Stdout, "hello Zoé".into())]);
    }

    #[test]
    fn carriage_return_keeps_the_last_rewrite() {
        assert_eq!(line_text(b"10%\r50%\r100%\r\n", false), "100%");
    }

    #[cfg(windows)]
    #[test]
    fn invalid_utf8_falls_back_to_the_oem_code_page() {
        use windows_sys::Win32::Globalization::GetOEMCP;
        if unsafe { GetOEMCP() } == 850 {
            // 0x82 is "é" in code page 850.
            assert_eq!(decode(b"caf\x82", false), "café");
        }
    }

    #[cfg(windows)]
    #[test]
    fn stop_kills_the_whole_tree() {
        // cmd starts ping, which would run ~30 s on its own.
        let mut process = spawn("ping -n 30 127.0.0.1 > nul");
        std::thread::sleep(Duration::from_millis(500));
        let started = Instant::now();
        process.stop();
        while process.try_wait().is_none() {
            assert!(started.elapsed() < Duration::from_secs(5), "not stopped");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
