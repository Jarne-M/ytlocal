//! egui front end. The UI only renders state and sends requests; it never
//! waits on a process, the network or the disk (the config is read once at
//! startup; every save happens on a background task).

mod history;
mod input;
mod options;
mod queue;
mod settings;
mod thumbs;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::backend::{Backend, Event, Request};
use crate::binaries::{Package, ToolReport, download};
use crate::config::history::{History, HistoryEntry, now_unix};
use crate::config::{Config, Mode};
use crate::queue::job::{JobEvent, JobId, JobState};
use crate::ytdlp::url;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Queue,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BannerKind {
    Info,
    Success,
    Error,
}

struct Banner {
    text: String,
    kind: BannerKind,
    until: Instant,
}

pub struct App {
    backend: Backend,
    config: Config,
    saved_config: Config,
    config_changed_at: Option<Instant>,
    history: History,

    tools: Option<ToolReport>,
    tool_tasks: HashMap<Package, download::Progress>,
    tool_messages: HashMap<Package, Result<String, String>>,
    show_tools: bool,
    show_settings: bool,

    input: input::InputState,
    trim_start: String,
    trim_end: String,

    jobs: Vec<queue::JobView>,
    thumbs: thumbs::Thumbs,
    next_id: JobId,
    tab: Tab,
    banner: Option<Banner>,

    folder_pick: Option<(Mode, mpsc::Receiver<Option<PathBuf>>)>,
    was_focused: bool,
    last_clipboard_link: Option<String>,
    quit_dialog: bool,
    quitting_since: Option<Instant>,
    /// Links given on the command line, added once the tools are ready.
    startup_links: Option<String>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: tokio::runtime::Handle,
        config: Config,
        startup_links: Vec<String>,
    ) -> Self {
        let backend = Backend::start(&rt, Some(cc.egui_ctx.clone()), config.concurrency);
        style(&cc.egui_ctx);
        Self {
            backend,
            saved_config: config.clone(),
            config,
            config_changed_at: None,
            history: History::load(),
            tools: None,
            tool_tasks: HashMap::new(),
            tool_messages: HashMap::new(),
            show_tools: false,
            show_settings: false,
            input: input::InputState::default(),
            trim_start: String::new(),
            trim_end: String::new(),
            jobs: Vec::new(),
            thumbs: thumbs::Thumbs::default(),
            next_id: 1,
            tab: Tab::Queue,
            banner: None,
            folder_pick: None,
            was_focused: true,
            last_clipboard_link: None,
            quit_dialog: false,
            quitting_since: None,
            startup_links: (!startup_links.is_empty()).then(|| startup_links.join("\n")),
        }
    }

    /// yt-dlp and ffmpeg were found and work.
    pub fn tools_ready(&self) -> bool {
        self.tools.as_ref().is_some_and(ToolReport::required_ready)
    }

    /// (active, done, failed-or-cancelled) jobs in the queue list.
    pub fn job_counts(&self) -> (usize, usize, usize) {
        let done = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Done(_) | JobState::Skipped(_)))
            .count();
        let bad = self
            .jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Failed(_) | JobState::Cancelled))
            .count();
        (self.active_jobs(), done, bad)
    }

    /// Paths of finished downloads, oldest first.
    pub fn finished_files(&self) -> Vec<PathBuf> {
        self.jobs
            .iter()
            .filter_map(|j| j.output().map(PathBuf::from))
            .collect()
    }

    fn banner(&mut self, kind: BannerKind, text: impl Into<String>) {
        self.banner = Some(Banner {
            text: text.into(),
            kind,
            until: Instant::now()
                + Duration::from_secs(if kind == BannerKind::Error { 12 } else { 6 }),
        });
    }

    fn active_jobs(&self) -> usize {
        self.jobs.iter().filter(|j| !j.state.is_finished()).count()
    }

    fn handle_events(&mut self) {
        while let Some(event) = self.backend.try_recv() {
            match event {
                Event::Tools(report) => {
                    if !report.required_ready() {
                        self.show_tools = true;
                    }
                    self.tools = Some(*report);
                }
                Event::ToolProgress { pkg, progress } => {
                    self.tool_tasks.insert(pkg, progress);
                }
                Event::ToolFinished { pkg, result } => {
                    self.tool_tasks.remove(&pkg);
                    match &result {
                        Ok(msg) => self.banner(BannerKind::Success, msg.clone()),
                        Err(e) => self.banner(BannerKind::Error, format!("{}: {e}", pkg.label())),
                    }
                    self.tool_messages.insert(pkg, result);
                }
                Event::Job { id, event } => self.job_event(id, event),
                Event::QueueIdle { done, failed } => self.queue_finished(done, failed),
                Event::Preview { req, result } => self.input.preview_result(req, result),
                Event::Thumbnail { url, bytes } => self.thumbs.loaded(url, bytes),
                Event::Clipboard { text, auto } => self.clipboard(text, auto),
                Event::SaveFailed(e) => self.banner(BannerKind::Error, e),
            }
        }
    }

    fn job_event(&mut self, id: JobId, event: JobEvent) {
        let Some(job) = self.jobs.iter_mut().find(|j| j.spec.id == id) else {
            return;
        };
        match event {
            JobEvent::State(state) => {
                if !matches!(state, JobState::Downloading) {
                    job.fraction = None;
                    job.speed = None;
                    job.eta = None;
                }
                if let JobState::Done(path) = &state {
                    self.history.push(HistoryEntry {
                        title: job.spec.title.clone(),
                        url: job.spec.url.clone(),
                        path: path.clone(),
                        format: job.spec.format.label(),
                        finished_at: now_unix(),
                    });
                    self.backend
                        .send(Request::SaveHistory(self.history.clone()));
                }
                job.state = state;
            }
            JobEvent::Progress {
                fraction,
                speed,
                eta,
            } => {
                job.fraction = fraction;
                job.speed = speed;
                job.eta = eta;
            }
        }
    }

    fn queue_finished(&mut self, done: usize, failed: usize) {
        if self.quitting_since.is_some() {
            return;
        }
        let single = (done + failed == 1)
            .then(|| self.jobs.iter().rev().find(|j| j.state.is_finished()))
            .flatten();
        let (title, body) = match (single, failed) {
            (Some(job), 0) => ("Download finished".to_string(), job.spec.title.clone()),
            (Some(job), _) => ("Download failed".to_string(), job.spec.title.clone()),
            (None, 0) => (
                "Downloads finished".to_string(),
                format!("{done} files saved"),
            ),
            (None, _) => (
                "Downloads finished".to_string(),
                format!("{done} saved, {failed} failed or cancelled"),
            ),
        };
        let kind = if failed == 0 {
            BannerKind::Success
        } else {
            BannerKind::Error
        };
        self.banner(kind, format!("{title}: {body}"));
        if self.config.notifications {
            self.backend.send(Request::Toast { title, body });
        }
    }

    fn clipboard(&mut self, text: Option<String>, auto: bool) {
        let Some(text) = text else {
            if !auto {
                self.banner(BannerKind::Info, "The clipboard doesn't contain text.");
            }
            return;
        };
        if auto {
            let Some(link) = url::find_youtube_link(&text) else {
                return;
            };
            if self.last_clipboard_link.as_deref() == Some(link.as_str())
                || self.input.contains(&link)
            {
                return;
            }
            self.last_clipboard_link = Some(link.clone());
            self.input.add_links(&link, &self.backend);
            self.banner(
                BannerKind::Info,
                "YouTube link found in the clipboard and added.",
            );
        } else {
            let added = self.input.add_links(&text, &self.backend);
            if added == 0 && self.input.error.is_none() {
                self.banner(BannerKind::Info, "No new YouTube links in the clipboard.");
            }
        }
    }

    fn pick_folder(&mut self, mode: Mode, ctx: &egui::Context) {
        if self.folder_pick.is_some() {
            return;
        }
        let start = match mode {
            Mode::Audio => self.config.audio_dir.clone(),
            Mode::Video => self.config.video_dir.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        // The native dialog blocks its thread, so it gets its own.
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new().set_directory(&start).pick_folder();
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.folder_pick = Some((mode, rx));
    }

    fn poll_folder_pick(&mut self) {
        let Some((mode, rx)) = &self.folder_pick else {
            return;
        };
        match rx.try_recv() {
            Ok(picked) => {
                if let Some(dir) = picked {
                    match mode {
                        Mode::Audio => self.config.audio_dir = dir,
                        Mode::Video => self.config.video_dir = dir,
                    }
                }
                self.folder_pick = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.folder_pick = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn autosave(&mut self, ctx: &egui::Context) {
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            let size = [rect.width().round(), rect.height().round()];
            if self.config.window_size != Some(size) && size[0] > 100.0 && size[1] > 100.0 {
                self.config.window_size = Some(size);
            }
        }
        if self.config == self.saved_config {
            self.config_changed_at = None;
            return;
        }
        let changed = *self.config_changed_at.get_or_insert_with(Instant::now);
        if changed.elapsed() >= AUTOSAVE_DELAY {
            self.saved_config = self.config.clone();
            self.config_changed_at = None;
            self.backend
                .send(Request::SaveConfig(Box::new(self.config.clone())));
        } else {
            ctx.request_repaint_after(AUTOSAVE_DELAY);
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if let Some(since) = self.quitting_since {
            if self.active_jobs() == 0 || since.elapsed() > Duration::from_secs(4) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            // Persist right away; the background saver may not get to run.
            if self.config != self.saved_config {
                let _ = self.config.save();
            }
            if self.active_jobs() > 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.quit_dialog = true;
            }
        }
    }

    fn quit_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.quit_dialog {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("quit_modal")).show(ui.ctx(), |ui| {
            ui.set_width(340.0);
            ui.heading("Downloads in progress");
            ui.label(format!(
                "{} download(s) are still running. Quitting cancels them and deletes their partial files.",
                self.active_jobs()
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel downloads and quit").clicked() {
                    self.backend.send(Request::CancelAll);
                    self.quitting_since = Some(Instant::now());
                    self.quit_dialog = false;
                }
                if ui.button("Keep downloading").clicked() {
                    self.quit_dialog = false;
                }
            });
        });
        if modal.should_close() {
            self.quit_dialog = false;
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(egui::RichText::new("YtLocal").strong());
            ui.label(egui::RichText::new("YouTube to MP3/MP4, all local").weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_settings, "⚙ Settings");
                let tools_label = match &self.tools {
                    None => "Tools …".to_string(),
                    Some(t) if !t.required_ready() => "⚠ Tools".to_string(),
                    Some(t) => match &t.ytdlp {
                        crate::binaries::ToolState::Ready { version, .. } => {
                            format!("yt-dlp {version}")
                        }
                        _ => "Tools".to_string(),
                    },
                };
                ui.toggle_value(&mut self.show_tools, tools_label)
                    .on_hover_text("yt-dlp, ffmpeg and deno status, install and update");
            });
        });
        if self.show_tools {
            ui.separator();
            tools::tools_panel(self, ui);
        }
        let mut dismiss = false;
        if let Some(b) = &self.banner {
            if Instant::now() > b.until {
                dismiss = true;
            } else {
                let color = match b.kind {
                    BannerKind::Info => ui.visuals().text_color(),
                    BannerKind::Success => egui::Color32::from_rgb(90, 190, 110),
                    BannerKind::Error => egui::Color32::from_rgb(230, 110, 90),
                };
                ui.horizontal(|ui| {
                    ui.colored_label(color, &b.text);
                    dismiss = ui.small_button("✖").clicked();
                });
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
        }
        if dismiss {
            self.banner = None;
        }
    }
}

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.poll_folder_pick();
        self.thumbs.request_missing(&self.backend);
        if self.tools_ready()
            && let Some(links) = self.startup_links.take()
        {
            self.input.add_links(&links, &self.backend);
        }

        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused && self.config.clipboard_watch {
            self.backend.send(Request::ReadClipboard { auto: true });
        }
        self.was_focused = focused;

        self.autosave(ctx);
        self.handle_close(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header").show(ui, |ui| self.header(ui));
        egui::Panel::left("new_download")
            .default_size(430.0)
            .size_range(340.0..=700.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        input::input_section(self, ui);
                        ui.add_space(6.0);
                        options::options_section(self, ui);
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let active = self.active_jobs();
                let q = if active > 0 {
                    format!("Queue ({active})")
                } else {
                    "Queue".into()
                };
                ui.selectable_value(&mut self.tab, Tab::Queue, q);
                ui.selectable_value(&mut self.tab, Tab::History, "History");
            });
            ui.separator();
            match self.tab {
                Tab::Queue => queue::queue_panel(self, ui),
                Tab::History => history::history_panel(self, ui),
            }
        });
        if self.show_settings {
            settings::settings_window(self, ui.ctx());
        }
        self.quit_dialog(ui);
    }
}
