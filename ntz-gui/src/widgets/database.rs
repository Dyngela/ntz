use eframe::egui;

/// Database tool window. Data sources will come from ntz-core once it exposes them.
#[derive(Default)]
pub struct DatabaseExplorer {
    data_sources: Vec<DataSource>,
}

pub struct DataSource {
    pub name: String,
}

impl DatabaseExplorer {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Database");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled(false, egui::Button::new("➕"))
                    .on_disabled_hover_text("New data source (not implemented yet)");
            });
        });
        ui.separator();

        if self.data_sources.is_empty() {
            ui.weak("No data sources");
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for source in &self.data_sources {
                    egui::CollapsingHeader::new(format!("🗄 {}", source.name)).show(ui, |ui| {
                        ui.weak("Schemas");
                    });
                }
            });
    }
}
