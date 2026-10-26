//! Run tool window: launches run configurations and shows their output, one tab per
//! configuration, plus the run toolbar (configuration selector, Run, Stop).

mod edit_dialog;
mod output;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use ntz_core::run::{self, RunConfig, RunningProcess};

use self::edit_dialog::EditDialog;
use self::output::Output;

/// Height of the input line under a running program's output.
const INPUT_HEIGHT: f32 = 26.0;

pub struct RunTool {
    root: PathBuf,
    custom: Vec<RunConfig>,
    detected: Detected,
    /// Name of the configuration selected in the toolbar.
    selected: Option<String>,
    tabs: Vec<RunTab>,
    active: usize,
    editor: Option<EditDialog>,
    /// Problem reading or writing `.ntz/run.json`.
    config_error: Option<String>,
}

enum Detected {
    Loading(Receiver<Vec<RunConfig>>),
    Ready(Vec<RunConfig>),
}

struct RunTab {
    config: RunConfig,
    process: Option<RunningProcess>,
    output: Output,
    /// What the user is typing for the program's stdin.
    input: String,
    status: Status,
    started: Instant,
}

enum Status {
    Running,
    Finished { code: Option<i32>, took: Duration },
    Stopped,
    FailedToStart(String),
}

impl RunTool {
    pub fn new(root: PathBuf, ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let detect_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(run::detect(&detect_root));
            ctx.request_repaint();
        });
        let (custom, config_error) = match run::load(&root) {
            Ok(custom) => (custom, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        Self {
            root,
            custom,
            detected: Detected::Loading(rx),
            selected: None,
            tabs: Vec::new(),
            active: 0,
            editor: None,
            config_error,
        }
    }

    /// Custom configurations first, then detected ones.
    fn configs(&self) -> impl Iterator<Item = &RunConfig> {
        let detected = match &self.detected {
            Detected::Ready(detected) => detected.as_slice(),
            Detected::Loading(_) => &[],
        };
        self.custom.iter().chain(detected)
    }

    fn selected_config(&self) -> Option<&RunConfig> {
        let name = self.selected.as_ref()?;
        self.configs().find(|c| &c.name == name)
    }

    pub fn is_running(&self, name: &str) -> bool {
        self.tabs
            .iter()
            .any(|t| t.config.name == name && matches!(t.status, Status::Running))
    }

    /// Call every frame: collects output and notices finished processes.
    pub fn update(&mut self, ctx: &egui::Context) {
        if let Detected::Loading(rx) = &self.detected
            && let Ok(detected) = rx.try_recv()
        {
            self.detected = Detected::Ready(detected);
        }
        if self.selected_config().is_none() {
            let first = self.configs().next().map(|c| c.name.clone());
            self.selected = first;
        }

        for tab in &mut self.tabs {
            let Some(process) = &mut tab.process else {
                continue;
            };
            for line in process.drain() {
                tab.output.push(line);
            }
            if matches!(tab.status, Status::Running)
                && let Some(status) = process.try_wait()
            {
                tab.status = Status::Finished {
                    code: status.code(),
                    took: tab.started.elapsed(),
                };
            }
        }
        if self
            .tabs
            .iter()
            .any(|t| matches!(t.status, Status::Running))
        {
            // Exits don't wake egui up by themselves.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Starts `config` in its tab (restarting it if it's running) and makes that tab active.
    fn run(&mut self, config: RunConfig, ctx: &egui::Context) {
        let index = match self.tabs.iter().position(|t| t.config.name == config.name) {
            Some(index) => index,
            None => {
                self.tabs.push(RunTab {
                    config: config.clone(),
                    process: None,
                    output: Output::default(),
                    input: String::new(),
                    status: Status::Stopped,
                    started: Instant::now(),
                });
                self.tabs.len() - 1
            }
        };
        let tab = &mut self.tabs[index];
        tab.process = None; // Kills a previous run still going.
        tab.config = config;
        tab.output.clear();
        tab.input.clear();
        tab.started = Instant::now();
        let notify = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match RunningProcess::spawn(&tab.config, &self.root, notify) {
            Ok(process) => {
                tab.process = Some(process);
                tab.status = Status::Running;
            }
            Err(e) => tab.status = Status::FailedToStart(e.to_string()),
        }
        self.active = index;
    }

    fn stop(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index)
            && matches!(tab.status, Status::Running)
        {
            if let Some(process) = &mut tab.process {
                process.stop();
            }
            tab.status = Status::Stopped;
        }
    }

    /// Run toolbar, for the right end of the menu bar (laid out right to left).
    /// Returns `true` when a run was started, so the tool window can be shown.
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let selected = self.selected_config().cloned();
        let running = selected.as_ref().is_some_and(|c| self.is_running(&c.name));

        let shortcut_run = ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::SHIFT,
                egui::Key::F10,
            ))
        });
        let shortcut_stop = ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::F2,
            ))
        });

        let stop = ui
            .add_enabled(running, egui::Button::new("■"))
            .on_hover_text("Stop (Ctrl+F2)")
            .clicked();
        let run_label = if running { "↻" } else { "▶" };
        let run = ui
            .add_enabled(selected.is_some(), egui::Button::new(run_label))
            .on_hover_text(if running {
                "Rerun (Shift+F10)"
            } else {
                "Run (Shift+F10)"
            })
            .clicked();

        let mut edit = false;
        let text = selected
            .as_ref()
            .map_or("Add configuration…", |c| c.name.as_str())
            .to_owned();
        egui::ComboBox::from_id_salt("run_configuration")
            .selected_text(text)
            .width(180.0)
            .show_ui(ui, |ui| {
                for config in &self.custom {
                    ui.selectable_value(
                        &mut self.selected,
                        Some(config.name.clone()),
                        &config.name,
                    );
                }
                match &self.detected {
                    Detected::Loading(_) => {
                        ui.weak("Detecting…");
                    }
                    Detected::Ready(detected) if !detected.is_empty() => {
                        ui.separator();
                        for config in detected {
                            ui.selectable_value(
                                &mut self.selected,
                                Some(config.name.clone()),
                                &config.name,
                            )
                            .on_hover_text(&config.command);
                        }
                    }
                    Detected::Ready(_) => {}
                }
                ui.separator();
                edit = ui.button("Edit configurations…").clicked();
            });
        if edit {
            self.editor = Some(EditDialog::new(&self.custom, self.selected.as_deref()));
        }

        let selected = self.selected_config().cloned();
        if (stop || shortcut_stop)
            && let Some(config) = &selected
            && let Some(index) = self.tabs.iter().position(|t| t.config.name == config.name)
        {
            self.stop(index);
        }
        if (run || shortcut_run)
            && let Some(config) = selected
        {
            self.run(config, ui.ctx());
            return true;
        }
        false
    }

    /// The tool window's contents.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Claim the panel's whole area, once: an egui panel otherwise shrinks to its content
        // (e.g. to one line before anything has run).
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(rect));

        if let Some(e) = &self.config_error {
            ui.colored_label(ui.visuals().error_fg_color, e.as_str());
        }
        if self.tabs.is_empty() {
            ui.weak("Nothing has run yet: pick a configuration in the toolbar and press ▶.");
            return;
        }
        self.active = self.active.min(self.tabs.len() - 1);

        let mut rerun = None;
        let mut stop = None;
        let mut close = None;
        ui.horizontal(|ui| {
            ui.strong("Run");
            ui.separator();
            for (index, tab) in self.tabs.iter().enumerate() {
                let icon = if matches!(tab.status, Status::Running) {
                    "▶ "
                } else {
                    ""
                };
                if ui
                    .selectable_label(index == self.active, format!("{icon}{}", tab.config.name))
                    .on_hover_text(&tab.config.command)
                    .clicked()
                {
                    self.active = index;
                }
                if ui.small_button("🗙").on_hover_text("Close tab").clicked() {
                    close = Some(index);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tab = &mut self.tabs[self.active];
                if ui.button("🗑").on_hover_text("Clear output").clicked() {
                    tab.output.clear();
                }
                let running = matches!(tab.status, Status::Running);
                if ui
                    .add_enabled(running, egui::Button::new("■"))
                    .on_hover_text("Stop")
                    .clicked()
                {
                    stop = Some(self.active);
                }
                if ui.button("↻").on_hover_text("Rerun").clicked() {
                    rerun = Some(tab.config.clone());
                }
            });
        });
        ui.separator();

        if let Some(index) = stop {
            self.stop(index);
        }
        if let Some(config) = rerun {
            self.run(config, ui.ctx());
        }
        if let Some(index) = close {
            self.tabs.remove(index); // Dropping the process kills it.
            if self.tabs.is_empty() {
                return;
            }
            self.active = self.active.min(self.tabs.len() - 1);
        }

        let active = self.active;
        let tab = &mut self.tabs[active];
        let accepts_input = matches!(tab.status, Status::Running)
            && tab.process.as_ref().is_some_and(|p| p.accepts_input());
        let area = ui.available_rect_before_wrap();
        let (output_rect, input_rect) = if accepts_input {
            area.split_top_bottom_at_y(area.bottom() - INPUT_HEIGHT)
        } else {
            (area, egui::Rect::NOTHING)
        };
        output::show(
            &mut ui.new_child(egui::UiBuilder::new().max_rect(output_rect)),
            &tab.output,
            &status_line(&tab.status),
            active,
        );
        if accepts_input {
            input_ui(
                &mut ui.new_child(egui::UiBuilder::new().max_rect(input_rect)),
                tab,
                active,
            );
        }
    }

    /// The "Edit configurations" dialog, when open; call every frame.
    pub fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let detected = match &self.detected {
            Detected::Ready(detected) => detected.as_slice(),
            Detected::Loading(_) => &[],
        };
        match editor.show(ctx, detected) {
            edit_dialog::Outcome::Open => {}
            edit_dialog::Outcome::Cancel => self.editor = None,
            edit_dialog::Outcome::Save(custom, selected) => match run::save(&self.root, &custom) {
                Ok(()) => {
                    self.custom = custom;
                    if selected.is_some() {
                        self.selected = selected;
                    }
                    self.config_error = None;
                    self.editor = None;
                }
                Err(e) => editor.set_error(e),
            },
        }
    }
}

/// The line the user types the program's input in: Enter sends it, Ctrl+D ends the input.
fn input_ui(ui: &mut egui::Ui, tab: &mut RunTab, index: usize) {
    let Some(process) = &mut tab.process else {
        return;
    };
    ui.horizontal_centered(|ui| {
        ui.monospace("›");
        let edit = ui.add(
            egui::TextEdit::singleline(&mut tab.input)
                .id(egui::Id::new(("run_input", index)))
                .font(egui::TextStyle::Monospace)
                .hint_text("Input for the program: Enter to send, Ctrl+D to end the input")
                .desired_width(f32::INFINITY),
        );
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let line = std::mem::take(&mut tab.input);
            process.send_input(&line);
            tab.output.push_input(&line);
            edit.request_focus();
        }
        if edit.has_focus()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D))
        {
            process.close_input();
        }
    });
}

fn status_line(status: &Status) -> String {
    match status {
        Status::Running => String::new(),
        Status::Finished {
            code: Some(code),
            took,
        } => format!(
            "Process finished with exit code {code} ({:.1} s)",
            took.as_secs_f32()
        ),
        Status::Finished { code: None, took } => {
            format!("Process terminated ({:.1} s)", took.as_secs_f32())
        }
        Status::Stopped => "Process stopped".to_owned(),
        Status::FailedToStart(e) => format!("Failed to start: {e}"),
    }
}
