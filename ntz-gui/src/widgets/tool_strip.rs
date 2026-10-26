use eframe::egui;

/// Vertical strip of toggle buttons, one per tool window: (icon, name, visible).
pub fn tool_strip(ui: &mut egui::Ui, tools: &mut [(&str, &str, &mut bool)]) {
    ui.vertical_centered(|ui| {
        for (icon, name, visible) in tools {
            let button =
                egui::Button::selectable(**visible, *icon).min_size(egui::vec2(24.0, 24.0));
            if ui.add(button).on_hover_text(*name).clicked() {
                **visible = !**visible;
            }
        }
    });
}
