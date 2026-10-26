use eframe::egui;

use crate::widgets::{DatabaseExplorer, Editor, FileTree, RunTool, Terminal, tool_strip};

pub struct NtzApp {
    project: FileTree,
    database: DatabaseExplorer,
    editor: Editor,
    terminal: Terminal,
    run: RunTool,
    show_project: bool,
    show_database: bool,
    show_terminal: bool,
    show_run: bool,
    /// Bottom tool windows as of the previous frame, to tell which one was just opened.
    bottom_shown: (bool, bool),
}

impl NtzApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| ".".into());
        Self {
            terminal: Terminal::new(cc, root.clone()),
            run: RunTool::new(root.clone(), &cc.egui_ctx),
            editor: Editor::new(root.clone(), &cc.egui_ctx),
            project: FileTree::new(root),
            database: DatabaseExplorer::default(),
            show_project: true,
            show_database: true,
            show_terminal: false,
            show_run: false,
            bottom_shown: (false, false),
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Save").clicked() {
                    self.editor.save_active();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_project, "Project");
                ui.checkbox(&mut self.show_database, "Database");
                ui.checkbox(&mut self.show_terminal, "Terminal");
                ui.checkbox(&mut self.show_run, "Run");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.run.toolbar_ui(ui) {
                    self.show_run = true;
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match self.editor.active_document() {
                Some(doc) => ui.label(doc.path.display().to_string()),
                None => ui.weak("No file open"),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(doc) = self.editor.active_document() {
                    ui.label(format!("{} lines", doc.text.lines().count()));
                }
                if let Some(status) = self.editor.status() {
                    ui.separator();
                    ui.label(status);
                }
            });
        });
    }

    /// Terminal and Run share the bottom area, like JetBrains tool windows: opening one
    /// closes the other.
    fn one_bottom_tool_window(&mut self) {
        let (terminal_was, run_was) = self.bottom_shown;
        if self.show_run && !run_was {
            self.show_terminal = false;
        } else if self.show_terminal && !terminal_was {
            self.show_run = false;
        }
        self.bottom_shown = (self.show_terminal, self.show_run);
    }
}

impl eframe::App for NtzApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let save = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);
        if ui.input_mut(|i| i.consume_shortcut(&save)) {
            self.editor.save_active();
        }
        // Every frame, even with the panels hidden: boots Cmder in the background, collects
        // run output.
        self.terminal.update(ui.ctx());
        self.run.update(ui.ctx());

        // Outer chrome first: panels added earlier claim space from the edges.
        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));

        egui::Panel::left("left_tool_strip")
            .resizable(false)
            .exact_size(32.0)
            .show(ui, |ui| {
                tool_strip(
                    ui,
                    &mut [
                        ("📁", "Project", &mut self.show_project),
                        ("🖥", "Terminal", &mut self.show_terminal),
                        ("▶", "Run", &mut self.show_run),
                    ],
                )
            });
        egui::Panel::right("right_tool_strip")
            .resizable(false)
            .exact_size(32.0)
            .show(ui, |ui| {
                tool_strip(ui, &mut [("🗄", "Database", &mut self.show_database)])
            });
        self.one_bottom_tool_window();

        // Tool windows; each one added takes its side of what's left. The project tree comes
        // first so it gets the full height; the bottom ones then span everything to its
        // right, under the database. egui's own resize edge doesn't work next to the
        // terminal's embedded native window, so the terminal sizes itself (its header is the
        // grip).
        egui::Panel::left("project_panel")
            .default_size(260.0)
            .min_size(120.0)
            .show_collapsible(ui, &mut self.show_project, |ui| {
                if let Some(path) = self.project.ui(ui) {
                    self.editor.open(path);
                }
            });
        egui::Panel::bottom("terminal_panel")
            .resizable(false)
            .exact_size(self.terminal.height())
            .show_collapsible(ui, &mut self.show_terminal, |ui| self.terminal.ui(ui));
        egui::Panel::bottom("run_panel")
            .default_size(260.0)
            .min_size(80.0)
            .show_collapsible(ui, &mut self.show_run, |ui| self.run.ui(ui));
        egui::Panel::right("database_panel")
            .default_size(260.0)
            .min_size(120.0)
            .show_collapsible(ui, &mut self.show_database, |ui| self.database.ui(ui));

        egui::CentralPanel::default_margins().show(ui, |ui| self.editor.ui(ui));

        self.run.dialog_ui(ui.ctx());
        self.terminal.end_frame(ui.ctx());
    }
}
