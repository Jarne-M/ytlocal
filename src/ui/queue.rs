//! The download queue list.

use std::path::{Path, PathBuf};

use super::App;
use crate::backend::Request;
use crate::queue::job::{JobId, JobSpec, JobState};

pub struct JobView {
    pub spec: JobSpec,
    pub state: JobState,
    pub fraction: Option<f32>,
    pub speed: Option<f64>,
    pub eta: Option<u64>,
    pub show_details: bool,
}

impl JobView {
    pub fn new(spec: JobSpec) -> Self {
        Self {
            spec,
            state: JobState::Queued,
            fraction: None,
            speed: None,
            eta: None,
            show_details: false,
        }
    }

    pub fn output(&self) -> Option<&Path> {
        match &self.state {
            JobState::Done(p) | JobState::Skipped(p) => Some(p),
            _ => None,
        }
    }
}

/// Open a file with its default app.
pub fn open_file(path: &Path) {
    let _ = crate::process::std_command("explorer.exe")
        .arg(path)
        .spawn();
}

/// Open Explorer with the file selected (or the folder itself).
pub fn open_folder(path: &Path) {
    let mut cmd = crate::process::std_command("explorer.exe");
    if path.is_file() {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(format!("/select,\"{}\"", path.display()));
        }
        #[cfg(not(windows))]
        cmd.arg(path);
    } else {
        cmd.arg(path);
    }
    let _ = cmd.spawn();
}

pub fn human_bytes(b: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn human_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

enum Action {
    Cancel(JobId),
    Retry(JobId),
    Remove(JobId),
}

pub fn queue_panel(app: &mut App, ui: &mut egui::Ui) {
    let active = app.jobs.iter().filter(|j| !j.state.is_finished()).count();
    let finished = app.jobs.len() - active;
    ui.horizontal(|ui| {
        ui.heading("Queue");
        ui.label(egui::RichText::new(format!("{active} active, {finished} finished")).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(finished > 0, egui::Button::new("Clear finished"))
                .clicked()
            {
                app.jobs.retain(|j| !j.state.is_finished());
            }
            if ui
                .add_enabled(active > 0, egui::Button::new("Cancel all"))
                .clicked()
            {
                app.backend.send(Request::CancelAll);
            }
        });
    });
    ui.separator();

    if app.jobs.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Nothing queued yet. Paste a link above to start.").weak(),
            );
        });
        return;
    }

    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for job in app.jobs.iter_mut().rev() {
                job_row(ui, job, &mut actions, &mut app.thumbs);
                ui.separator();
            }
        });

    for action in actions {
        match action {
            Action::Cancel(id) => app.backend.send(Request::Cancel(id)),
            Action::Retry(id) => {
                if let Some(job) = app.jobs.iter_mut().find(|j| j.spec.id == id) {
                    *job = JobView::new(job.spec.clone());
                    app.backend.send(Request::Enqueue(vec![job.spec.clone()]));
                }
            }
            Action::Remove(id) => app.jobs.retain(|j| j.spec.id != id),
        }
    }
}

fn job_row(
    ui: &mut egui::Ui,
    job: &mut JobView,
    actions: &mut Vec<Action>,
    thumbs: &mut super::thumbs::Thumbs,
) {
    ui.horizontal(|ui| {
        let thumb_url = job
            .spec
            .video_id
            .as_deref()
            .map(crate::ytdlp::url::Link::thumbnail_url);
        thumbs.show(ui, thumb_url.as_deref(), egui::vec2(96.0, 54.0));

        ui.vertical(|ui| {
            ui.add(egui::Label::new(egui::RichText::new(&job.spec.title).strong()).truncate());
            let mut meta = job.spec.format.label();
            if let Some(c) = &job.spec.channel {
                meta = format!("{c}  ·  {meta}");
            }
            ui.add(egui::Label::new(egui::RichText::new(meta).weak().small()).truncate());

            let (color, status) = status_text(ui, job);
            ui.horizontal(|ui| {
                ui.colored_label(color, status);
                if let JobState::Failed(err) = &job.state
                    && !err.detail.is_empty()
                    && ui
                        .small_button(if job.show_details {
                            "Hide details"
                        } else {
                            "Details"
                        })
                        .clicked()
                {
                    job.show_details = !job.show_details;
                }
            });
            if job.state.is_active() {
                let bar = egui::ProgressBar::new(job.fraction.unwrap_or(0.0))
                    .desired_height(6.0)
                    .desired_width(ui.available_width() - 150.0);
                ui.add(if job.fraction.is_none() {
                    bar.animate(true)
                } else {
                    bar
                });
            }
            if let (true, JobState::Failed(err)) = (job.show_details, &job.state) {
                egui::ScrollArea::vertical()
                    .id_salt(("details", job.spec.id))
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&err.detail).monospace().small())
                                .wrap(),
                        );
                    });
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let id = job.spec.id;
            if job.state.is_finished() {
                if ui
                    .small_button("✖")
                    .on_hover_text("Remove from list")
                    .clicked()
                {
                    actions.push(Action::Remove(id));
                }
            } else if ui.small_button("Cancel").clicked() {
                actions.push(Action::Cancel(id));
            }
            if matches!(job.state, JobState::Failed(_) | JobState::Cancelled)
                && ui.small_button("Retry").clicked()
            {
                actions.push(Action::Retry(id));
            }
            if let Some(path) = job.output().map(Path::to_path_buf) {
                if ui
                    .small_button("Folder")
                    .on_hover_text("Show in Explorer")
                    .clicked()
                {
                    open_folder(&path);
                }
                if ui.small_button("Open").clicked() {
                    open_file(&path);
                }
            }
        });
    });
}

fn status_text(ui: &egui::Ui, job: &JobView) -> (egui::Color32, String) {
    let weak = ui.visuals().weak_text_color();
    let ok = egui::Color32::from_rgb(90, 190, 110);
    let bad = egui::Color32::from_rgb(230, 110, 90);
    match &job.state {
        JobState::Downloading => {
            let mut s = String::from("Downloading");
            if let Some(f) = job.fraction {
                s += &format!("  {:.0}%", f * 100.0);
            }
            if let Some(sp) = job.speed {
                s += &format!("  ·  {}/s", human_bytes(sp));
            }
            if let Some(eta) = job.eta {
                s += &format!("  ·  {} left", human_eta(eta));
            }
            (ui.visuals().text_color(), s)
        }
        JobState::Converting(what) => {
            let pct = job
                .fraction
                .map(|f| format!("  {:.0}%", f * 100.0))
                .unwrap_or_default();
            (ui.visuals().text_color(), format!("{what}…{pct}"))
        }
        JobState::Done(p) => (ok, format!("Done  ·  {}", file_name(p))),
        JobState::Skipped(p) => (
            weak,
            format!("Skipped, already exists  ·  {}", file_name(p)),
        ),
        JobState::Failed(e) => (bad, e.to_string()),
        other => (weak, other.label().to_string()),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| PathBuf::from(p).display().to_string())
}
