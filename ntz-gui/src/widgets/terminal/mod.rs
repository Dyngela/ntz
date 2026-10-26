//! Terminal tool window: Cmder (ConEmu) sessions embedded as native child windows.
//!
//! egui draws the header (session tabs, also the resize grip) and status messages. Each
//! session's ConEmu docks itself inside a Win32 host window (`-InsideWnd`) that is moved over
//! the rest of the panel every frame; only the active session's host is visible.
//!
//! Cmder is located and the first session started as soon as the app opens, in the
//! background. If Cmder is missing, the user is asked whether to install it the first time
//! the panel is opened.

mod install_dialog;
mod native;
mod session;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;
use raw_window_handle::HasWindowHandle;

use self::install_dialog::DialogAction;
use self::session::{Session, Shell};
use crate::cmder::{self, Progress};

const DEFAULT_HEIGHT: f32 = 260.0;
const MIN_HEIGHT: f32 = 80.0;
const HEADER_HEIGHT: f32 = 26.0;
/// Size of a session's host until the panel has been shown once (physical pixels).
const DEFAULT_HOST_RECT: [i32; 4] = [0, 0, 1000, 240];
/// How long the panel's area must stay unchanged before ConEmu is shown or resized.
const SETTLE_DELAY: Duration = Duration::from_millis(150);

pub struct Terminal {
    parent: Option<native::Hwnd>,
    cwd: PathBuf,
    cmder: Cmder,
    sessions: Vec<Session>,
    active: usize,
    /// Sessions opened so far per shell, for tab titles ("Cmder", "Cmder (2)", …).
    opened: [u32; Shell::ALL.len()],
    /// Last failure to open a session.
    error: Option<String>,
    /// Panel height in points; the header is the grip that changes it.
    height: f32,
    /// Where the active host was last shown; new sessions start there (hidden).
    last_rect: Option<[i32; 4]>,
    /// Latest area seen for the active host, and since when it hasn't changed.
    target: Option<([i32; 4], Instant)>,
    shown_this_frame: bool,
}

/// Where we are in finding (or installing) Cmder.
enum Cmder {
    NotStarted,
    Locating(Receiver<Option<PathBuf>>),
    /// Not found; waiting for the user to choose a folder and authorize the install.
    NeedsInstall {
        dialog_open: bool,
        /// Whether the dialog was already shown once on its own.
        prompted: bool,
        /// Install location, as typed in the dialog.
        dest: String,
    },
    Installing {
        dest: PathBuf,
        events: Receiver<InstallEvent>,
        progress: Option<Progress>,
    },
    Ready(PathBuf),
    Failed(String),
}

enum InstallEvent {
    Progress(Progress),
    Done(Result<(), String>),
}

impl Terminal {
    pub fn new(window: &impl HasWindowHandle, cwd: PathBuf) -> Self {
        Self {
            parent: native::hwnd_of(window),
            cwd,
            cmder: Cmder::NotStarted,
            sessions: Vec::new(),
            active: 0,
            opened: [0; Shell::ALL.len()],
            error: None,
            height: DEFAULT_HEIGHT,
            last_rect: None,
            target: None,
            shown_this_frame: false,
        }
    }

    /// Height to give the panel, in points.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Call at the start of every frame, whether the panel is shown or not: this is what
    /// boots Cmder in the background and notices sessions whose shell exited.
    pub fn update(&mut self, ctx: &egui::Context) {
        match &mut self.cmder {
            Cmder::NotStarted => {
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(cmder::locate());
                    ctx.request_repaint();
                });
                self.cmder = Cmder::Locating(rx);
            }
            Cmder::Locating(rx) => match rx.try_recv() {
                Ok(Some(root)) => self.cmder_ready(root),
                Ok(None) => {
                    let dest =
                        cmder::managed_dir().map_or_else(String::new, |d| d.display().to_string());
                    self.cmder = Cmder::NeedsInstall {
                        dialog_open: false,
                        prompted: false,
                        dest,
                    };
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.cmder = Cmder::Failed("Looking for Cmder failed unexpectedly".into());
                }
            },
            Cmder::Installing {
                dest,
                events,
                progress,
            } => loop {
                match events.try_recv() {
                    Ok(InstallEvent::Progress(p)) => *progress = Some(p),
                    Ok(InstallEvent::Done(Ok(()))) => {
                        let dest = dest.clone();
                        self.cmder_ready(dest);
                        break;
                    }
                    Ok(InstallEvent::Done(Err(e))) => {
                        self.cmder = Cmder::Failed(e);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.cmder = Cmder::Failed("The Cmder install stopped unexpectedly".into());
                        break;
                    }
                }
            },
            Cmder::NeedsInstall { .. } | Cmder::Ready(_) | Cmder::Failed(_) => {}
        }

        // A shell that exits closes its tab, like in JetBrains IDEs.
        self.sessions.retain_mut(Session::is_alive);
        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        if !self.sessions.is_empty() {
            // Exits don't wake egui up by themselves.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.shown_this_frame = true;

        // Claim exactly the panel's area, once: the panel keeps its size whatever is shown
        // (claiming any more would make it grow a little on every repaint).
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        let (header, body) = rect.split_top_bottom_at_y(rect.top() + HEADER_HEIGHT);

        self.header_ui(ui, header);
        self.body_ui(
            &mut ui.new_child(egui::UiBuilder::new().max_rect(body)),
            body,
        );
        self.install_dialog(ui.ctx());
    }

    /// Call once per frame after the panels: hides the hosts that weren't drawn and keeps
    /// keyboard focus out of them.
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        let mut hidden_has_focus = false;
        for (index, session) in self.sessions.iter_mut().enumerate() {
            if !(self.shown_this_frame && index == self.active) {
                session.host.hide();
                hidden_has_focus |= session.host.has_focus();
            }
        }
        // ConEmu takes focus when it starts, even hidden. And clicks inside a terminal go to
        // ConEmu, so any press egui sees is outside it: egui gets the keyboard back.
        if (hidden_has_focus || ctx.input(|i| i.pointer.any_pressed()))
            && let Some(parent) = self.parent
        {
            native::focus(parent);
        }
        self.shown_this_frame = false;
    }

    fn cmder_ready(&mut self, root: PathBuf) {
        self.cmder = Cmder::Ready(root.clone());
        if self.open(Shell::Cmder) {
            cmder::remember(&root);
        }
    }

    /// Opens a new session and makes it the active one.
    fn open(&mut self, shell: Shell) -> bool {
        let Cmder::Ready(root) = &self.cmder else {
            return false;
        };
        let Some(parent) = self.parent else {
            self.error = Some("No native window handle to embed the terminal into".into());
            return false;
        };
        let count = self.opened[shell as usize] + 1;
        let title = match count {
            1 => shell.name().to_owned(),
            n => format!("{} ({n})", shell.name()),
        };
        let rect = self.last_rect.unwrap_or(DEFAULT_HOST_RECT);
        match Session::spawn(parent, root, &self.cwd, shell, title, rect) {
            Ok(session) => {
                self.opened[shell as usize] = count;
                self.sessions.push(session);
                self.active = self.sessions.len() - 1;
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    /// Session tabs and the "+" menu. The header's background is the panel's resize grip.
    fn header_ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let grip = ui.interact(rect, ui.id().with("terminal_grip"), egui::Sense::drag());
        if grip.dragged() {
            let max = (ui.ctx().content_rect().height() - 150.0).max(MIN_HEIGHT);
            self.height = (self.height - grip.drag_delta().y).clamp(MIN_HEIGHT, max);
        }
        if grip.hovered() || grip.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }

        let layout = egui::Layout::left_to_right(egui::Align::Center);
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
        ui.strong("Terminal");
        ui.separator();

        let mut close = None;
        for (index, session) in self.sessions.iter().enumerate() {
            if ui
                .selectable_label(index == self.active, session.title.as_str())
                .clicked()
            {
                self.active = index;
            }
            if ui
                .small_button("🗙")
                .on_hover_text("Close session")
                .clicked()
            {
                close = Some(index);
            }
        }
        if let Some(index) = close {
            self.sessions.remove(index);
            self.active = self.active.min(self.sessions.len().saturating_sub(1));
        }

        let mut open = None;
        if let Cmder::Ready(root) = &self.cmder {
            ui.menu_button("➕", |ui| {
                for shell in Shell::ALL.into_iter().filter(|s| s.is_available(root)) {
                    if ui.button(shell.name()).clicked() {
                        open = Some(shell);
                    }
                }
            })
            .response
            .on_hover_text("New session");
        }
        if let Some(shell) = open {
            self.open(shell);
        }

        if let Some(e) = &self.error
            && !self.sessions.is_empty()
        {
            ui.colored_label(ui.visuals().error_fg_color, e.as_str());
        }
    }

    fn body_ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        match &mut self.cmder {
            Cmder::NotStarted => {}
            Cmder::Locating(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for Cmder…");
                });
            }
            Cmder::NeedsInstall {
                dialog_open,
                prompted,
                ..
            } => {
                // Ask once on its own, the first time the panel is opened.
                if !*prompted {
                    *prompted = true;
                    *dialog_open = true;
                }
                ui.label("Cmder is not installed.");
                if ui.button("Install Cmder…").clicked() {
                    *dialog_open = true;
                }
            }
            Cmder::Installing { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing Cmder…");
                });
            }
            Cmder::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                if ui.button("Retry").clicked() {
                    self.cmder = Cmder::NotStarted;
                    ui.ctx().request_repaint();
                }
            }
            Cmder::Ready(_) => match self.sessions.get_mut(self.active) {
                Some(session) => {
                    let ppp = ui.ctx().pixels_per_point();
                    let physical = [
                        (rect.min.x * ppp).round() as i32,
                        (rect.min.y * ppp).round() as i32,
                        (rect.width() * ppp).round() as i32,
                        (rect.height() * ppp).round() as i32,
                    ];
                    // Only resize ConEmu once the area is stable: resizing it on every frame
                    // of the slide-in animation or of a header drag (especially while it is
                    // still starting) can crash it. Meanwhile the session stays hidden.
                    let now = Instant::now();
                    match self.target {
                        Some((target, since)) if target == physical => {
                            let waited = now - since;
                            if waited >= SETTLE_DELAY {
                                session.host.place(physical);
                                self.last_rect = Some(physical);
                            } else {
                                ui.ctx().request_repaint_after(SETTLE_DELAY - waited);
                            }
                        }
                        _ => {
                            self.target = Some((physical, now));
                            session.host.hide();
                            ui.ctx().request_repaint_after(SETTLE_DELAY);
                        }
                    }
                }
                None => {
                    if let Some(e) = &self.error {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    ui.label("No terminal session.");
                    if ui.button("New Cmder session").clicked() {
                        self.open(Shell::Cmder);
                    }
                }
            },
        }
    }

    /// Asks where and whether to install before downloading anything, then shows progress.
    fn install_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::new("install_cmder"));
        let mut action = None;
        match &mut self.cmder {
            Cmder::NeedsInstall {
                dialog_open: true,
                dest,
                ..
            } => {
                let response = modal.show(ctx, |ui| {
                    action = install_dialog::choose_location_ui(ui, dest);
                });
                // Escape or a click outside closes the dialog.
                if response.should_close() && action.is_none() {
                    action = Some(DialogAction::Dismiss);
                }
            }
            // No way to dismiss while installing: the panel would have nothing to show.
            Cmder::Installing { dest, progress, .. } => {
                modal.show(ctx, |ui| {
                    install_dialog::install_progress_ui(ui, dest, progress.as_ref())
                });
            }
            _ => return,
        }

        match action {
            Some(DialogAction::Install(dest)) => {
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                let target = dest.clone();
                std::thread::spawn(move || {
                    let result = cmder::install(&target, |p| {
                        let _ = tx.send(InstallEvent::Progress(p));
                        ctx.request_repaint();
                    });
                    let _ = tx.send(InstallEvent::Done(result));
                    ctx.request_repaint();
                });
                self.cmder = Cmder::Installing {
                    dest,
                    events: rx,
                    progress: None,
                };
            }
            Some(DialogAction::UseExisting(root)) => self.cmder_ready(root),
            Some(DialogAction::Dismiss) => {
                if let Cmder::NeedsInstall { dialog_open, .. } = &mut self.cmder {
                    *dialog_open = false;
                }
            }
            None => {}
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // On exit, wait for every ConEmu here (bounded) rather than in detached threads that
        // would die with the process before they could kill a stuck one.
        let processes = self
            .sessions
            .iter_mut()
            .filter_map(Session::shut_down)
            .collect();
        session::wait_or_kill(processes, Duration::from_secs(2));
    }
}
