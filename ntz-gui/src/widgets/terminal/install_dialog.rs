//! The "Install Cmder?" dialog's contents.

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::cmder::{self, Progress};

pub enum DialogAction {
    Install(PathBuf),
    UseExisting(PathBuf),
    Dismiss,
}

/// First step: pick a folder, then confirm.
pub fn choose_location_ui(ui: &mut egui::Ui, dest: &mut String) -> Option<DialogAction> {
    ui.set_width(480.0);
    ui.heading("Install Cmder?");
    ui.add_space(8.0);
    ui.label(
        "The Terminal tool window embeds Cmder, which wasn't found on this machine. \
         ntz can download Cmder mini (about 12 MB) from its official GitHub releases.",
    );
    ui.add_space(12.0);

    ui.strong("Install location");
    ui.horizontal(|ui| {
        let browse_width = 80.0;
        ui.add(
            egui::TextEdit::singleline(dest)
                .desired_width(ui.available_width() - browse_width - ui.spacing().item_spacing.x),
        );
        let browse = egui::Button::new("Browse…");
        if ui
            .add_sized([browse_width, ui.spacing().interact_size.y], browse)
            .clicked()
            && let Some(picked) = pick_folder(dest)
        {
            *dest = picked.display().to_string();
        }
    });

    let path = PathBuf::from(dest.trim());
    let existing = cmder::is_cmder(&path);
    let check = if existing {
        Ok(())
    } else {
        cmder::check_install_dir(&path)
    };
    match &check {
        Ok(()) if existing => ui.weak("Cmder is already installed here: ntz will use it as is."),
        Ok(()) => ui.weak("Only this folder is written to; you can delete it at any time."),
        Err(e) => ui.colored_label(ui.visuals().error_fg_color, e.as_str()),
    };
    ui.add_space(12.0);

    let mut action = None;
    ui.horizontal(|ui| {
        let label = if existing {
            "Use this Cmder"
        } else {
            "Install"
        };
        if ui
            .add_enabled(check.is_ok(), egui::Button::new(label))
            .clicked()
        {
            action = Some(if existing {
                DialogAction::UseExisting(path)
            } else {
                DialogAction::Install(path)
            });
        }
        if ui.button("Not now").clicked() {
            action = Some(DialogAction::Dismiss);
        }
    });
    action
}

/// Native folder picker. Picking an empty folder or an existing Cmder uses it directly;
/// any other folder is taken as the parent, and Cmder goes in a `cmder` folder inside it.
fn pick_folder(current: &str) -> Option<PathBuf> {
    let current = Path::new(current.trim());
    let start = current.ancestors().find(|dir| dir.is_dir());
    let mut dialog = rfd::FileDialog::new().set_title("Choose where to install Cmder");
    if let Some(start) = start {
        dialog = dialog.set_directory(start);
    }
    let picked = dialog.pick_folder()?;
    let is_empty = std::fs::read_dir(&picked).is_ok_and(|mut entries| entries.next().is_none());
    Some(if is_empty || cmder::is_cmder(&picked) {
        picked
    } else {
        picked.join("cmder")
    })
}

/// Second step: the install's progress.
pub fn install_progress_ui(ui: &mut egui::Ui, dest: &Path, progress: Option<&Progress>) {
    ui.set_width(480.0);
    ui.heading("Installing Cmder");
    ui.add_space(8.0);
    ui.label("Installing in:");
    ui.code(dest.display().to_string());
    ui.add_space(12.0);

    let (fraction, text) = match progress {
        None => (0.0, "Connecting…".to_owned()),
        Some(Progress::Downloading {
            done,
            total: Some(total),
        }) => (
            *done as f32 / *total as f32,
            format!("Downloading {:.1} / {:.1} MB", mb(*done), mb(*total)),
        ),
        Some(Progress::Downloading { done, total: None }) => {
            (0.0, format!("Downloading {:.1} MB", mb(*done)))
        }
        Some(Progress::Extracting { done, total }) => (
            *done as f32 / *total as f32,
            format!("Extracting {done} / {total} files"),
        ),
    };
    ui.add(egui::ProgressBar::new(fraction).text(text).animate(true));
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}
