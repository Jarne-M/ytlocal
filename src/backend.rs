//! The bridge between the GUI thread and background work. The UI sends
//! [`Request`]s; everything slow runs on the tokio runtime and reports back
//! through [`Event`]s, each of which also wakes the UI for a repaint.

use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;

use crate::binaries::{self, Package, ToolReport, download};

pub enum Request {
    RefreshTools,
    Install(Package),
    UpdateYtDlp,
}

#[derive(Debug)]
pub enum Event {
    Tools(Box<ToolReport>),
    ToolProgress {
        pkg: Package,
        progress: download::Progress,
    },
    ToolFinished {
        pkg: Package,
        result: Result<String, String>,
    },
}

/// Sends events to the UI and asks egui to repaint.
#[derive(Clone)]
pub struct EventSink {
    tx: std::sync::mpsc::Sender<Event>,
    repaint: Option<egui::Context>,
}

impl EventSink {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }
}

/// State shared between the request loop and the tasks it spawns.
pub struct Shared {
    pub tools: RwLock<ToolReport>,
    pub http: reqwest::Client,
}

pub struct Backend {
    tx: mpsc::UnboundedSender<Request>,
    events: std::sync::mpsc::Receiver<Event>,
}

impl Backend {
    /// Start the backend on `rt`. `repaint` is the egui context to wake, if any.
    pub fn start(rt: &tokio::runtime::Handle, repaint: Option<egui::Context>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (etx, events) = std::sync::mpsc::channel();
        let sink = EventSink { tx: etx, repaint };
        let shared = Arc::new(Shared {
            tools: RwLock::new(ToolReport::default()),
            http: download::http_client(),
        });
        rt.spawn(run(rx, sink, shared));
        Self { tx, events }
    }

    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Blocking receive, for headless use.
    pub fn recv(&self) -> Option<Event> {
        self.events.recv().ok()
    }
}

async fn refresh_tools(shared: &Shared, sink: &EventSink) {
    let report = binaries::detect_all().await;
    *shared.tools.write().unwrap() = report.clone();
    sink.send(Event::Tools(Box::new(report)));
}

async fn run(mut rx: mpsc::UnboundedReceiver<Request>, sink: EventSink, shared: Arc<Shared>) {
    while let Some(req) = rx.recv().await {
        match req {
            Request::RefreshTools => {
                let (shared, sink) = (shared.clone(), sink.clone());
                tokio::spawn(async move { refresh_tools(&shared, &sink).await });
            }
            Request::Install(pkg) => {
                let (shared, sink) = (shared.clone(), sink.clone());
                tokio::spawn(async move {
                    let s = sink.clone();
                    let progress = move |progress| s.send(Event::ToolProgress { pkg, progress });
                    let result = download::install(pkg, &shared.http, &progress).await;
                    sink.send(Event::ToolFinished {
                        pkg,
                        result: result.map_err(|e| format!("{e:#}")),
                    });
                    refresh_tools(&shared, &sink).await;
                });
            }
            Request::UpdateYtDlp => {
                let (shared, sink) = (shared.clone(), sink.clone());
                tokio::spawn(async move {
                    let pkg = Package::YtDlp;
                    let current = shared
                        .tools
                        .read()
                        .unwrap()
                        .ytdlp
                        .path()
                        .map(|p| p.to_path_buf());
                    let s = sink.clone();
                    let progress = move |progress| s.send(Event::ToolProgress { pkg, progress });
                    let result =
                        download::update_ytdlp(current.as_deref(), &shared.http, &progress).await;
                    sink.send(Event::ToolFinished {
                        pkg,
                        result: result.map_err(|e| format!("{e:#}")),
                    });
                    refresh_tools(&shared, &sink).await;
                });
            }
        }
    }
}
