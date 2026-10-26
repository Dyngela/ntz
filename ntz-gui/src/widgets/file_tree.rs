use std::path::{Path, PathBuf};

use eframe::egui;

/// Project tool window: lazily-loaded view of the filesystem under `root`.
pub struct FileTree {
    root: Node,
}

struct Node {
    path: PathBuf,
    name: String,
    is_dir: bool,
    /// Loaded the first time the directory is expanded.
    children: Option<Vec<Node>>,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        Self {
            root: Node {
                path: root,
                name,
                is_dir: true,
                children: None,
            },
        }
    }

    /// Returns the file the user double-clicked this frame, if any.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        ui.heading("Project");
        ui.separator();
        let mut clicked = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            show_node(ui, &mut self.root, true, &mut clicked);
        });
        clicked
    }
}

fn show_node(
    ui: &mut egui::Ui,
    node: &mut Node,
    default_open: bool,
    clicked: &mut Option<PathBuf>,
) {
    if node.is_dir {
        egui::CollapsingHeader::new(format!("📁 {}", node.name))
            .id_salt(&node.path)
            .default_open(default_open)
            .show(ui, |ui| {
                let children = node.children.get_or_insert_with(|| read_dir(&node.path));
                for child in children {
                    show_node(ui, child, false, clicked);
                }
            });
    } else if ui
        .selectable_label(false, format!("📄 {}", node.name))
        .double_clicked()
    {
        *clicked = Some(node.path.clone());
    }
}

fn read_dir(path: &Path) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node> = entries
        .flatten()
        .map(|entry| Node {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type().is_ok_and(|t| t.is_dir()),
            children: None,
        })
        .collect();
    // Directories first, then case-insensitive alphabetical, like JetBrains.
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    nodes
}
