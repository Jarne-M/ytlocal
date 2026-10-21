//! The bridge between the GUI thread and background work. The UI sends
//! [`Request`]s; everything slow runs on the tokio runtime and reports back
//! through [`Event`]s, each of which also wakes the UI for a repaint.

use std::sync::{Arc, RwLock};

use tokio::sync::{Semaphore, mpsc};

use crate::binaries::{self, Package, ToolReport, download};
use crate::config::{Config, history::History};
use crate::queue::job::{JobEvent, JobId, JobSpec, JobState};
use crate::queue::{Scheduler, runner};
use crate::ytdlp::{self, errors::UserError, metadata::Preview};

pub enum Request {
    RefreshTools,
    Install(Package),
    UpdateYtDlp,
    Enqueue(Vec<JobSpec>),
    Cancel(JobId),
    CancelAll,
    SetConcurrency(usize),
    /// Fetch metadata for a link; `req` is echoed back in the event.
    Preview {
        req: u64,
        url: String,
        playlist: bool,
    },
    Thumbnail(String),
    /// Read the clipboard; `auto` marks a focus-triggered check.
    ReadClipboard {
        auto: bool,
    },
    Toast {
        title: String,
        body: String,
    },
    SaveConfig(Box<Config>),
    SaveHistory(History),
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
    Job {
        id: JobId,
        event: JobEvent,
    },
    /// The queue ran empty; counts are for the jobs since it was last idle.
    QueueIdle {
        done: usize,
        failed: usize,
    },
    Preview {
        req: u64,
        result: Result<Preview, UserError>,
    },
    Thumbnail {
        url: String,
        bytes: Option<Arc<[u8]>>,
    },
    Clipboard {
        text: Option<String>,
        auto: bool,
    },
    /// Saving settings or history failed.
    SaveFailed(String),
}

enum SaveJob {
    Config(Box<Config>),
    History(History),
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
struct Shared {
    tools: RwLock<ToolReport>,
    http: reqwest::Client,
    previews: Semaphore,
    thumbnails: Semaphore,
    /// Saves go through one task so they're written in order.
    saves: mpsc::UnboundedSender<SaveJob>,
}

async fn saver(mut rx: mpsc::UnboundedReceiver<SaveJob>, sink: EventSink) {
    while let Some(job) = rx.recv().await {
        let result = tokio::task::spawn_blocking(move || match job {
            SaveJob::Config(c) => c.save().map_err(|e| format!("Couldn't save settings: {e}")),
            SaveJob::History(h) => h.save().map_err(|e| format!("Couldn't save history: {e}")),
        })
        .await;
        if let Ok(Err(e)) = result {
            sink.send(Event::SaveFailed(e));
        }
    }
}

enum Msg {
    Ui(Request),
    /// `ok`: `Some(true)` done/skipped, `Some(false)` failed, `None` cancelled.
    JobDone {
        id: JobId,
        ok: Option<bool>,
    },
}

pub struct Backend {
    tx: mpsc::UnboundedSender<Msg>,
    events: std::sync::mpsc::Receiver<Event>,
}

impl Backend {
    /// Start the backend on `rt`. `repaint` is the egui context to wake, if any.
    pub fn start(
        rt: &tokio::runtime::Handle,
        repaint: Option<egui::Context>,
        concurrency: usize,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (etx, events) = std::sync::mpsc::channel();
        let sink = EventSink { tx: etx, repaint };
        let (saves, saves_rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            tools: RwLock::new(ToolReport::default()),
            http: download::http_client(),
            previews: Semaphore::new(3),
            thumbnails: Semaphore::new(6),
            saves,
        });
        rt.spawn(saver(saves_rx, sink.clone()));
        rt.spawn(runner::clean_stale_work_dirs());
        rt.spawn(run(
            rx,
            tx.clone(),
            sink,
            shared,
            Scheduler::new(concurrency),
        ));
        let backend = Self { tx, events };
        backend.send(Request::RefreshTools);
        backend
    }

    pub fn send(&self, request: Request) {
        let _ = self.tx.send(Msg::Ui(request));
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

async fn run(
    mut rx: mpsc::UnboundedReceiver<Msg>,
    tx: mpsc::UnboundedSender<Msg>,
    sink: EventSink,
    shared: Arc<Shared>,
    mut queue: Scheduler,
) {
    // Finished since the queue was last idle; cancelled jobs aren't counted.
    let (mut done, mut failed) = (0usize, 0usize);
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Ui(req) => handle_request(req, &sink, &shared, &mut queue),
            Msg::JobDone { id, ok } => {
                queue.finished(id);
                match ok {
                    Some(true) => done += 1,
                    Some(false) => failed += 1,
                    None => {}
                }
            }
        }

        while let Some((spec, token)) = queue.start_next() {
            let (sink, tx) = (sink.clone(), tx.clone());
            let tools = shared.tools.read().unwrap().paths();
            tokio::spawn(async move {
                let id = spec.id;
                let s = sink.clone();
                let report = move |event| s.send(Event::Job { id, event });
                let state = runner::run(&spec, &tools, &token, &report).await;
                let ok = match state {
                    JobState::Done(_) | JobState::Skipped(_) => Some(true),
                    JobState::Cancelled => None,
                    _ => Some(false),
                };
                report(JobEvent::State(state));
                let _ = tx.send(Msg::JobDone { id, ok });
            });
        }
        if queue.is_idle() {
            if done + failed > 0 {
                sink.send(Event::QueueIdle { done, failed });
            }
            (done, failed) = (0, 0);
        }
    }
}

fn spawn_tool_task(pkg: Package, sink: &EventSink, shared: &Arc<Shared>, update: bool) {
    let (shared, sink) = (shared.clone(), sink.clone());
    tokio::spawn(async move {
        let s = sink.clone();
        let progress = move |progress| s.send(Event::ToolProgress { pkg, progress });
        let result = if update {
            let current = shared
                .tools
                .read()
                .unwrap()
                .ytdlp
                .path()
                .map(|p| p.to_path_buf());
            download::update_ytdlp(current.as_deref(), &shared.http, &progress).await
        } else {
            download::install(pkg, &shared.http, &progress).await
        };
        sink.send(Event::ToolFinished {
            pkg,
            result: result.map_err(|e| format!("{e:#}")),
        });
        refresh_tools(&shared, &sink).await;
    });
}

fn handle_request(req: Request, sink: &EventSink, shared: &Arc<Shared>, queue: &mut Scheduler) {
    match req {
        Request::RefreshTools => {
            let (shared, sink) = (shared.clone(), sink.clone());
            tokio::spawn(async move { refresh_tools(&shared, &sink).await });
        }
        Request::Install(pkg) => spawn_tool_task(pkg, sink, shared, false),
        Request::UpdateYtDlp => spawn_tool_task(Package::YtDlp, sink, shared, true),
        Request::Enqueue(specs) => {
            for spec in specs {
                sink.send(Event::Job {
                    id: spec.id,
                    event: JobEvent::State(JobState::Queued),
                });
                queue.enqueue(spec);
            }
        }
        Request::Cancel(id) => {
            if queue.cancel(id) {
                sink.send(Event::Job {
                    id,
                    event: JobEvent::State(JobState::Cancelled),
                });
            }
        }
        Request::CancelAll => {
            for id in queue.cancel_all() {
                sink.send(Event::Job {
                    id,
                    event: JobEvent::State(JobState::Cancelled),
                });
            }
        }
        Request::SetConcurrency(n) => queue.set_concurrency(n),
        Request::Preview { req, url, playlist } => {
            let (shared, sink) = (shared.clone(), sink.clone());
            tokio::spawn(async move {
                let _permit = shared.previews.acquire().await;
                let tools = shared.tools.read().unwrap().paths();
                let result = ytdlp::fetch_preview(&tools, &url, playlist).await;
                sink.send(Event::Preview { req, result });
            });
        }
        Request::Thumbnail(url) => {
            let (shared, sink) = (shared.clone(), sink.clone());
            tokio::spawn(async move {
                let _permit = shared.thumbnails.acquire().await;
                let bytes = async {
                    let resp = shared
                        .http
                        .get(&url)
                        .send()
                        .await
                        .ok()?
                        .error_for_status()
                        .ok()?;
                    resp.bytes().await.ok()
                }
                .await;
                sink.send(Event::Thumbnail {
                    url,
                    bytes: bytes.map(|b| Arc::from(b.as_ref())),
                });
            });
        }
        Request::ReadClipboard { auto } => {
            let sink = sink.clone();
            tokio::task::spawn_blocking(move || {
                let text = arboard::Clipboard::new()
                    .and_then(|mut c| c.get_text())
                    .ok();
                sink.send(Event::Clipboard { text, auto });
            });
        }
        Request::Toast { title, body } => {
            tokio::task::spawn_blocking(move || {
                let _ = crate::notify::toast(&title, &body);
            });
        }
        Request::SaveConfig(c) => {
            let _ = shared.saves.send(SaveJob::Config(c));
        }
        Request::SaveHistory(h) => {
            let _ = shared.saves.send(SaveJob::History(h));
        }
    }
}
