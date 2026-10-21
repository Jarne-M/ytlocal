//! Tool status and one-click install/update.

use super::App;
use crate::backend::Request;
use crate::binaries::{Package, ToolState};

/// Status line for a tool, prefixed with `prefix` (used for ffmpeg/ffprobe).
fn state_text(prefix: &str, state: &ToolState) -> (egui::RichText, Option<String>) {
    match state {
        ToolState::Ready { path, version } => (
            egui::RichText::new(format!("{prefix}✔ {}", short_version(version)))
                .color(egui::Color32::from_rgb(90, 190, 110)),
            Some(format!("{version}\n{}", path.display())),
        ),
        ToolState::Missing => (
            egui::RichText::new(format!("{prefix}✖ not found"))
                .color(egui::Color32::from_rgb(230, 110, 90)),
            None,
        ),
        ToolState::Broken { path, error } => (
            egui::RichText::new(format!("{prefix}⚠ does not run (corrupt?)"))
                .color(egui::Color32::from_rgb(230, 170, 60)),
            Some(format!("{}: {error}", path.display())),
        ),
    }
}

pub fn tools_panel(app: &mut App, ui: &mut egui::Ui) {
    let Some(report) = app.tools.clone() else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Looking for yt-dlp, ffmpeg and deno…");
        });
        return;
    };

    egui::Grid::new("tools_grid")
        .num_columns(4)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for pkg in [Package::YtDlp, Package::Ffmpeg, Package::Deno] {
                ui.strong(pkg.label());
                ui.vertical(|ui| {
                    for tool in pkg.tools() {
                        let prefix = if pkg.tools().len() > 1 {
                            format!("{}: ", tool.label())
                        } else {
                            String::new()
                        };
                        let (text, hover) = state_text(&prefix, report.get(*tool));
                        let label =
                            ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
                        if let Some(h) = hover {
                            label.on_hover_text(h);
                        }
                    }
                });

                let busy = app.tool_tasks.get(&pkg);
                ui.horizontal(|ui| {
                    if let Some(p) = busy {
                        let frac = p.total.map(|t| p.done as f32 / t.max(1) as f32);
                        let text = match p.total {
                            Some(t) => format!("{} {:.1}/{:.1} MB", p.stage, mb(p.done), mb(t)),
                            None if p.done > 0 => format!("{} {:.1} MB", p.stage, mb(p.done)),
                            None => p.stage.clone(),
                        };
                        let bar = egui::ProgressBar::new(frac.unwrap_or(0.0))
                            .desired_width(220.0)
                            .text(text);
                        ui.add(if frac.is_none() {
                            bar.animate(true)
                        } else {
                            bar
                        });
                    } else {
                        let ready = report.package_ready(pkg);
                        let label = if ready { "Reinstall" } else { "Download" };
                        if ui.button(label).clicked() {
                            app.backend.send(Request::Install(pkg));
                        }
                        if pkg == Package::YtDlp
                            && ready
                            && ui
                                .button("Update yt-dlp")
                                .on_hover_text(
                                    "YouTube changes often; update when downloads start failing",
                                )
                                .clicked()
                        {
                            app.backend.send(Request::UpdateYtDlp);
                        }
                    }
                });

                match app.tool_messages.get(&pkg) {
                    Some(Ok(msg)) => ui.label(egui::RichText::new(msg).weak()),
                    Some(Err(err)) => ui.colored_label(egui::Color32::from_rgb(230, 110, 90), err),
                    None if pkg == Package::Deno && !report.package_ready(pkg) => ui.label(
                        egui::RichText::new("Optional, recommended: helps yt-dlp with YouTube")
                            .weak(),
                    ),
                    None => ui.label(""),
                };
                ui.end_row();
            }
        });

    if ui.button("⟳ Re-check").clicked() {
        app.tools = None;
        app.backend.send(Request::RefreshTools);
    }
}

/// "N-127900-ge2798d5cad-20261021" (ffmpeg git builds) -> "N-127900"; others unchanged.
pub(super) fn short_version(v: &str) -> &str {
    match v.strip_prefix("N-") {
        Some(rest) => &v[..2 + rest.find('-').unwrap_or(rest.len())],
        None => v,
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
