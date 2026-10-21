//! egui front end. The UI only renders state and sends requests; it never
//! waits on a process, the network or the disk.

mod queue;
mod thumbs;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::backend::{Backend, Event, Request};
use crate::binaries::{Package, ToolReport, download};
use crate::queue::job::{ConflictPolicy, JobEvent, JobId, JobSpec};
use crate::ytdlp::command::{AudioFormat, Options, OutputFormat};
use crate::ytdlp::url::{self, Link};

pub struct App {
    backend: Backend,
    tools: Option<ToolReport>,
    tool_tasks: HashMap<Package, download::Progress>,
    /// Last result line per package, e.g. "Installed yt-dlp 2026.08.19".
    tool_messages: HashMap<Package, Result<String, String>>,
    show_tools: bool,

    url_text: String,
    input_error: Option<String>,
    format: OutputFormat,
    output_dir: PathBuf,

    jobs: Vec<queue::JobView>,
    thumbs: thumbs::Thumbs,
    next_id: JobId,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        let backend = Backend::start(&rt, Some(cc.egui_ctx.clone()), 2);
        Self {
            backend,
            tools: None,
            tool_tasks: HashMap::new(),
            tool_messages: HashMap::new(),
            show_tools: false,
            url_text: String::new(),
            input_error: None,
            format: OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            output_dir: dirs::audio_dir().unwrap_or_else(|| PathBuf::from(".")),
            jobs: Vec::new(),
            thumbs: thumbs::Thumbs::default(),
            next_id: 1,
        }
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
                    self.tool_messages.insert(pkg, result);
                }
                Event::Job { id, event } => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.spec.id == id) {
                        match event {
                            JobEvent::State(s) => {
                                if !matches!(s, crate::queue::job::JobState::Downloading) {
                                    job.fraction = None;
                                    job.speed = None;
                                    job.eta = None;
                                }
                                job.state = s;
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
                }
                Event::Thumbnail { url, bytes } => self.thumbs.loaded(url, bytes),
                _ => {}
            }
        }
    }

    fn start_downloads(&mut self) {
        let mut specs = Vec::new();
        let mut errors = Vec::new();
        for raw in url::split_input(&self.url_text) {
            match url::parse(&raw) {
                Ok(Link::Video { id } | Link::VideoInPlaylist { id, .. }) => {
                    specs.push(JobSpec {
                        id: self.next_id,
                        url: Link::video_url(&id),
                        video_id: Some(id.clone()),
                        title: id,
                        channel: None,
                        duration: None,
                        album: None,
                        format: self.format,
                        options: Options::default(),
                        output_dir: self.output_dir.clone(),
                        template: "%(title)s.%(ext)s".into(),
                        on_conflict: ConflictPolicy::Rename,
                    });
                    self.next_id += 1;
                }
                Ok(Link::Playlist { .. }) => {
                    errors.push(format!("{raw}: playlists are not supported yet"))
                }
                Err(e) => errors.push(format!("{raw}: {e}")),
            }
        }
        self.input_error = (!errors.is_empty()).then(|| errors.join("\n"));
        if !specs.is_empty() {
            self.jobs
                .extend(specs.iter().cloned().map(queue::JobView::new));
            self.backend.send(Request::Enqueue(specs));
            self.url_text.clear();
        }
    }

    fn input_panel(&mut self, ui: &mut egui::Ui) {
        ui.label("YouTube links (one per line)");
        ui.add(
            egui::TextEdit::multiline(&mut self.url_text)
                .hint_text("https://www.youtube.com/watch?v=…")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if let Some(e) = &self.input_error {
            ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
        }
        ui.horizontal(|ui| {
            if let OutputFormat::Audio { format, kbps } = &mut self.format {
                egui::ComboBox::from_id_salt("audio_format")
                    .selected_text(format.label())
                    .show_ui(ui, |ui| {
                        for f in AudioFormat::ALL {
                            ui.selectable_value(format, f, f.label());
                        }
                    });
                if format.has_bitrate() {
                    egui::ComboBox::from_id_salt("kbps")
                        .selected_text(format!("{kbps} kbps"))
                        .show_ui(ui, |ui| {
                            for k in crate::ytdlp::command::MP3_BITRATES {
                                ui.selectable_value(kbps, k, format!("{k} kbps"));
                            }
                        });
                }
            }
            ui.label(format!("→ {}", self.output_dir.display()));
            let ready = self.tools.as_ref().is_some_and(ToolReport::required_ready);
            if ui
                .add_enabled(
                    ready && !self.url_text.trim().is_empty(),
                    egui::Button::new("Download"),
                )
                .clicked()
            {
                self.start_downloads();
            }
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.thumbs.request_missing(&self.backend);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("YtLocal");
                ui.label(egui::RichText::new("YouTube to MP3/MP4, fully local").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_tools, "Tools");
                });
            });
            if self.show_tools {
                ui.separator();
                tools::tools_panel(self, ui);
            }
        });
        egui::Panel::top("input").show(ui, |ui| self.input_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| queue::queue_panel(self, ui));
    }
}
