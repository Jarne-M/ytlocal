//! The last 50 finished downloads.

use super::{App, BannerKind};
use crate::backend::Request;
use crate::config::history::now_unix;

fn ago(ts: u64) -> String {
    let secs = now_unix().saturating_sub(ts);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

pub fn history_panel(app: &mut App, ui: &mut egui::Ui) {
    if app.history.entries.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("Finished downloads show up here.").weak())
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Last {} downloads", app.history.entries.len())).weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Clear history").clicked() {
                app.history.entries.clear();
                app.backend.send(Request::SaveHistory(app.history.clone()));
            }
        });
    });
    let mut missing = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for e in &app.history.entries {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - 130.0);
                        ui.add(egui::Label::new(egui::RichText::new(&e.title).strong()).truncate());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!(
                                    "{}  ·  {}  ·  {}",
                                    e.format,
                                    ago(e.finished_at),
                                    e.path.display()
                                ))
                                .weak()
                                .small(),
                            )
                            .truncate(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Existence is only checked on click, never per frame.
                        if ui.small_button("Folder").clicked() {
                            if e.path.exists() {
                                super::queue::open_folder(&e.path);
                            } else {
                                missing = Some(e.path.clone());
                            }
                        }
                        if ui.small_button("Open").clicked() {
                            if e.path.exists() {
                                super::queue::open_file(&e.path);
                            } else {
                                missing = Some(e.path.clone());
                            }
                        }
                    });
                });
                ui.separator();
            }
        });
    if let Some(p) = missing {
        app.banner(
            BannerKind::Error,
            format!("File no longer exists: {}", p.display()),
        );
    }
}
