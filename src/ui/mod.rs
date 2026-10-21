//! egui front end. The UI only renders state and sends requests; it never
//! waits on a process, the network or the disk.

mod tools;

use std::collections::HashMap;

use crate::backend::{Backend, Event, Request};
use crate::binaries::{Package, ToolReport, download};

pub struct App {
    backend: Backend,
    tools: Option<ToolReport>,
    tool_tasks: HashMap<Package, download::Progress>,
    /// Last result line per package, e.g. "Installed yt-dlp 2026.08.19".
    tool_messages: HashMap<Package, Result<String, String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        let backend = Backend::start(&rt, Some(cc.egui_ctx.clone()));
        backend.send(Request::RefreshTools);
        Self {
            backend,
            tools: None,
            tool_tasks: HashMap::new(),
            tool_messages: HashMap::new(),
        }
    }

    fn handle_events(&mut self) {
        while let Some(event) = self.backend.try_recv() {
            match event {
                Event::Tools(report) => self.tools = Some(*report),
                Event::ToolProgress { pkg, progress } => {
                    self.tool_tasks.insert(pkg, progress);
                }
                Event::ToolFinished { pkg, result } => {
                    self.tool_tasks.remove(&pkg);
                    self.tool_messages.insert(pkg, result);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("YtLocal");
                ui.label(egui::RichText::new("YouTube to MP3/MP4, fully local").weak());
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            tools::tools_panel(self, ui);
        });
    }
}
