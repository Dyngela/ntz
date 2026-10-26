//! The "Run configurations" dialog: edit the project's custom configurations.

use eframe::egui;
use ntz_core::run::RunConfig;

pub struct EditDialog {
    /// Working copy, saved only on "Save".
    configs: Vec<RunConfig>,
    selected: Option<usize>,
    error: Option<String>,
}

pub enum Outcome {
    Open,
    Cancel,
    /// The configurations to save, and the one to select in the toolbar.
    Save(Vec<RunConfig>, Option<String>),
}

impl EditDialog {
    pub fn new(custom: &[RunConfig], selected: Option<&str>) -> Self {
        let selected = custom
            .iter()
            .position(|c| Some(c.name.as_str()) == selected)
            .or((!custom.is_empty()).then_some(0));
        Self {
            configs: custom.to_vec(),
            selected,
            error: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn show(&mut self, ctx: &egui::Context, detected: &[RunConfig]) -> Outcome {
        let mut outcome = Outcome::Open;
        let modal = egui::Modal::new(egui::Id::new("run_configurations")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading("Run configurations");
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    self.list_ui(ui, detected);
                });
                ui.separator();
                ui.vertical(|ui| self.form_ui(ui));
            });
            ui.add_space(8.0);

            let problem = self.validate();
            if let Some(e) = problem.as_ref().or(self.error.as_ref()) {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(problem.is_none(), egui::Button::new("Save"))
                    .clicked()
                {
                    let selected = self.selected.map(|i| self.configs[i].name.clone());
                    outcome = Outcome::Save(self.configs.clone(), selected);
                }
                if ui.button("Cancel").clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(outcome, Outcome::Open) {
            outcome = Outcome::Cancel;
        }
        outcome
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, detected: &[RunConfig]) {
        ui.strong("Custom");
        for (index, config) in self.configs.iter().enumerate() {
            let name = if config.name.is_empty() {
                "(unnamed)"
            } else {
                &config.name
            };
            if ui
                .selectable_label(self.selected == Some(index), name)
                .clicked()
            {
                self.selected = Some(index);
            }
        }
        ui.horizontal(|ui| {
            if ui.button("➕ Add").clicked() {
                self.add(RunConfig::new(self.unique_name("New configuration"), ""));
            }
            if ui
                .add_enabled(self.selected.is_some(), egui::Button::new("🗑 Remove"))
                .clicked()
                && let Some(index) = self.selected
            {
                self.configs.remove(index);
                self.selected = (!self.configs.is_empty()).then(|| index.saturating_sub(1));
            }
        });

        if !detected.is_empty() {
            ui.add_space(8.0);
            ui.strong("Detected");
            ui.weak("Copy one to customize it.");
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    for config in detected {
                        ui.horizontal(|ui| {
                            if ui.small_button("Copy").clicked() {
                                let copy = RunConfig {
                                    name: self.unique_name(&config.name),
                                    ..config.clone()
                                };
                                self.add(copy);
                            }
                            ui.label(&config.name).on_hover_text(&config.command);
                        });
                    }
                });
        }
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        let Some(config) = self.selected.and_then(|i| self.configs.get_mut(i)) else {
            ui.weak("Add a configuration, or copy a detected one.");
            return;
        };
        egui::Grid::new("run_config_form")
            .num_columns(2)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut config.name).desired_width(320.0));
                ui.end_row();

                ui.label("Command");
                ui.add(
                    egui::TextEdit::singleline(&mut config.command)
                        .code_editor()
                        .hint_text("cargo run -p my-app -- --verbose")
                        .desired_width(320.0),
                );
                ui.end_row();

                ui.label("Working directory");
                ui.add(
                    egui::TextEdit::singleline(&mut config.working_dir)
                        .hint_text("project root")
                        .desired_width(320.0),
                );
                ui.end_row();
            });
        ui.add_space(4.0);
        ui.weak(
            "The command runs in the system shell, as typed in a terminal. \
             The working directory is relative to the project root.",
        );
    }

    fn add(&mut self, config: RunConfig) {
        self.configs.push(config);
        self.selected = Some(self.configs.len() - 1);
    }

    fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.configs.iter().any(|c| c.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} ({n})"))
            .find(|name| !taken(name))
            .unwrap()
    }

    fn validate(&self) -> Option<String> {
        for (index, config) in self.configs.iter().enumerate() {
            if config.name.trim().is_empty() {
                return Some("Every configuration needs a name.".into());
            }
            if config.command.trim().is_empty() {
                return Some(format!("\"{}\" has no command.", config.name));
            }
            if self.configs[..index].iter().any(|c| c.name == config.name) {
                return Some(format!("Two configurations are named \"{}\".", config.name));
            }
        }
        None
    }
}
