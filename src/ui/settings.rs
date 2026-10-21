//! Settings window.

use super::App;
use crate::backend::Request;
use crate::config::{DEFAULT_TEMPLATE, normalize_template};
use crate::queue::MAX_CONCURRENCY;
use crate::queue::job::ConflictPolicy;

pub fn settings_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_settings;
    egui::Window::new("Settings")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            let c = &mut app.config;
            egui::Grid::new("settings")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    ui.label("File name");
                    ui.vertical(|ui| {
                        let edit = ui
                            .add(egui::TextEdit::singleline(&mut c.template).desired_width(260.0));
                        if edit.lost_focus() {
                            c.template = normalize_template(&c.template);
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(
                                    "yt-dlp template, e.g. %(uploader)s - %(title)s.%(ext)s",
                                )
                                .weak()
                                .small(),
                            );
                            if ui.small_button("Reset").clicked() {
                                c.template = DEFAULT_TEMPLATE.into();
                            }
                        });
                    });
                    ui.end_row();

                    ui.label("If the file exists");
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut c.on_conflict,
                            ConflictPolicy::Rename,
                            "Keep both (add number)",
                        );
                        ui.radio_value(&mut c.on_conflict, ConflictPolicy::Skip, "Skip");
                    });
                    ui.end_row();

                    ui.label("Parallel downloads");
                    if ui
                        .add(egui::Slider::new(&mut c.concurrency, 1..=MAX_CONCURRENCY))
                        .changed()
                    {
                        app.backend.send(Request::SetConcurrency(c.concurrency));
                    }
                    ui.end_row();

                    ui.label("Speed limit");
                    ui.horizontal(|ui| {
                        let mut limited = c.options.rate_limit.is_some();
                        if ui.checkbox(&mut limited, "").changed() {
                            c.options.rate_limit = limited.then_some(2048);
                        }
                        if let Some(kib) = &mut c.options.rate_limit {
                            ui.add(
                                egui::DragValue::new(kib)
                                    .range(50..=500_000)
                                    .speed(50)
                                    .suffix(" KB/s"),
                            );
                        } else {
                            ui.label(egui::RichText::new("unlimited").weak());
                        }
                    });
                    ui.end_row();

                    ui.label("Best video quality");
                    ui.vertical(|ui| {
                        ui.radio_value(
                            &mut c.options.prefer_h264,
                            true,
                            "Fast: best native H.264 (usually 1080p)",
                        );
                        ui.radio_value(
                            &mut c.options.prefer_h264,
                            false,
                            "Highest (1440p/4K, re-encoded to H.264; slow)",
                        );
                    });
                    ui.end_row();

                    ui.label("Clipboard");
                    ui.checkbox(
                        &mut c.clipboard_watch,
                        "Add YouTube links from the clipboard when the window gets focus",
                    );
                    ui.end_row();

                    ui.label("Notifications");
                    ui.checkbox(
                        &mut c.notifications,
                        "Windows notification when downloads finish",
                    );
                    ui.end_row();
                });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Open settings folder").clicked() {
                    let dir = crate::paths::app_data_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    super::queue::open_folder(&dir);
                }
                ui.label(egui::RichText::new("Settings are saved automatically.").weak());
            });
        });
    app.show_settings = open;
}
