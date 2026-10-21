//! Link input and the preview list (title, channel, duration, thumbnail;
//! playlist items with checkboxes) shown before anything is downloaded.

use super::{App, BannerKind};
use crate::backend::{Backend, Request};
use crate::queue::job::JobSpec;
use crate::ytdlp::errors::UserError;
use crate::ytdlp::metadata::{Preview, format_duration};
use crate::ytdlp::url::{self, Link};

enum Fetch {
    Idle,
    Loading(u64),
    Ready(Preview),
    Failed(UserError),
}

pub struct Item {
    link: Link,
    video: Fetch,
    playlist: Fetch,
    /// For watch links carrying `list=`: download the whole playlist instead.
    use_playlist: bool,
    selected: Vec<bool>,
}

impl Item {
    fn wants_playlist(&self) -> bool {
        matches!(self.link, Link::Playlist { .. }) || self.use_playlist
    }

    fn current(&self) -> &Fetch {
        if self.wants_playlist() {
            &self.playlist
        } else {
            &self.video
        }
    }

    fn key(&self) -> String {
        match &self.link {
            Link::Video { id } => id.clone(),
            Link::Playlist { list } => list.clone(),
            Link::VideoInPlaylist { id, list } => format!("{id}/{list}"),
        }
    }

    /// Number of downloads this item would add.
    fn count(&self) -> usize {
        match self.current() {
            Fetch::Ready(Preview::Video(_)) => 1,
            Fetch::Ready(Preview::Playlist(_)) => self.selected.iter().filter(|s| **s).count(),
            _ => 0,
        }
    }
}

#[derive(Default)]
pub struct InputState {
    pub text: String,
    pub error: Option<String>,
    items: Vec<Item>,
    next_req: u64,
}

impl InputState {
    fn request(&mut self, backend: &Backend, url: String, playlist: bool) -> Fetch {
        self.next_req += 1;
        backend.send(Request::Preview {
            req: self.next_req,
            url,
            playlist,
        });
        Fetch::Loading(self.next_req)
    }

    pub fn contains(&self, text: &str) -> bool {
        url::parse(text).is_ok_and(|l| self.items.iter().any(|i| i.link == l))
    }

    /// Parse links from `text` and start fetching previews. Returns how many were added.
    pub fn add_links(&mut self, text: &str, backend: &Backend) -> usize {
        let mut errors = Vec::new();
        let mut added = 0;
        for raw in url::split_input(text) {
            let link = match url::parse(&raw) {
                Ok(l) => l,
                Err(e) => {
                    errors.push(format!("{}: {e}", shorten(&raw, 60)));
                    continue;
                }
            };
            if self.items.iter().any(|i| i.link == link) {
                continue;
            }
            let (video, playlist) = match &link {
                Link::Video { id } | Link::VideoInPlaylist { id, .. } => (
                    self.request(backend, Link::video_url(id), false),
                    Fetch::Idle,
                ),
                Link::Playlist { list } => (
                    Fetch::Idle,
                    self.request(backend, Link::playlist_url(list), true),
                ),
            };
            self.items.push(Item {
                link,
                video,
                playlist,
                use_playlist: false,
                selected: Vec::new(),
            });
            added += 1;
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
        added
    }

    pub fn preview_result(&mut self, req: u64, result: Result<Preview, UserError>) {
        for item in &mut self.items {
            for slot in [&mut item.video, &mut item.playlist] {
                if matches!(slot, Fetch::Loading(r) if *r == req) {
                    if let Ok(Preview::Playlist(p)) = &result {
                        item.selected = p.entries.iter().map(|e| e.available).collect();
                    }
                    *slot = match result {
                        Ok(p) => Fetch::Ready(p),
                        Err(e) => Fetch::Failed(e),
                    };
                    return;
                }
            }
        }
    }

    pub fn total_selected(&self) -> usize {
        self.items.iter().map(Item::count).sum()
    }

    pub fn is_loading(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i.current(), Fetch::Loading(_)))
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Turn ready previews into jobs, remove them from the list, and queue them.
pub fn enqueue_ready(app: &mut App) {
    let format = app.config.output_format();
    let trim = match crate::ytdlp::command::parse_trim(&app.trim_start, &app.trim_end) {
        Ok(t) => t,
        Err(e) => {
            app.banner(BannerKind::Error, e);
            return;
        }
    };
    let mut options = app.config.options.clone();
    options.trim = trim;
    let output_dir = app.config.output_dir().to_path_buf();

    let mut specs: Vec<JobSpec> = Vec::new();
    let mut next_id = app.next_id;
    let mut new_spec = |url: String,
                        id: &str,
                        title: &str,
                        channel: Option<String>,
                        duration,
                        album: Option<String>| {
        let spec = JobSpec {
            id: next_id,
            url,
            video_id: Some(id.to_string()),
            title: title.to_string(),
            channel,
            duration,
            album,
            format,
            options: options.clone(),
            output_dir: output_dir.clone(),
            template: app.config.template.clone(),
            on_conflict: app.config.on_conflict,
        };
        next_id += 1;
        spec
    };

    app.input.items.retain(|item| {
        match item.current() {
            Fetch::Ready(Preview::Video(v)) => {
                specs.push(new_spec(
                    v.url.clone(),
                    &v.id,
                    &v.title,
                    v.channel.clone(),
                    v.duration,
                    None,
                ));
            }
            Fetch::Ready(Preview::Playlist(p)) => {
                for (entry, _) in p.entries.iter().zip(&item.selected).filter(|(_, s)| **s) {
                    specs.push(new_spec(
                        entry.url(),
                        &entry.id,
                        &entry.title,
                        entry.channel.clone().or_else(|| p.channel.clone()),
                        entry.duration,
                        Some(p.title.clone()),
                    ));
                }
            }
            _ => return true,
        }
        false
    });
    app.next_id = next_id;
    if specs.is_empty() {
        return;
    }
    app.jobs
        .extend(specs.iter().cloned().map(super::queue::JobView::new));
    app.backend.send(Request::Enqueue(specs));
    app.tab = super::Tab::Queue;
}

pub fn input_section(app: &mut App, ui: &mut egui::Ui) {
    ui.strong("Links");
    let edit = ui.add(
        egui::TextEdit::multiline(&mut app.input.text)
            .hint_text("Paste YouTube video or playlist links, one per line")
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    let ctrl_enter =
        edit.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        if ui
            .button("📋 Paste")
            .on_hover_text("Add links from the clipboard")
            .clicked()
        {
            app.backend.send(Request::ReadClipboard { auto: false });
        }
        let can_fetch = !app.input.text.trim().is_empty() && app.tools_ready();
        if ui
            .add_enabled(can_fetch, egui::Button::new("Fetch info"))
            .on_hover_text("Ctrl+Enter")
            .clicked()
            || (ctrl_enter && can_fetch)
        {
            let text = std::mem::take(&mut app.input.text);
            app.input.add_links(&text, &app.backend);
        }
        if !app.tools_ready() {
            ui.label(egui::RichText::new("Set up the tools first (top right)").weak());
        }
    });
    if let Some(e) = &app.input.error {
        ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
    }

    let mut remove = None;
    let mut fetch_playlist = None;
    let thumbs = &mut app.thumbs;
    for (idx, item) in app.input.items.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Link::VideoInPlaylist { .. } = item.link {
                ui.horizontal(|ui| {
                    ui.label("This link is part of a playlist:");
                    ui.radio_value(&mut item.use_playlist, false, "This video");
                    if ui
                        .radio_value(&mut item.use_playlist, true, "Whole playlist")
                        .clicked()
                        && matches!(item.playlist, Fetch::Idle)
                    {
                        fetch_playlist = Some(idx);
                    }
                });
            }
            ui.horizontal(|ui| {
                let thumb = match (&item.link, item.current()) {
                    (_, Fetch::Ready(Preview::Video(v))) => Some(v.thumbnail.clone()),
                    (_, Fetch::Ready(Preview::Playlist(p))) => {
                        p.entries.first().map(|e| e.thumbnail())
                    }
                    (Link::Video { id } | Link::VideoInPlaylist { id, .. }, _) => {
                        Some(Link::thumbnail_url(id))
                    }
                    _ => None,
                };
                thumbs.show(ui, thumb.as_deref(), egui::vec2(112.0, 63.0));
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() - 40.0);
                    match item.current() {
                        Fetch::Idle | Fetch::Loading(_) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Fetching info…");
                            });
                            ui.label(egui::RichText::new(item.key()).weak().small());
                        }
                        Fetch::Failed(e) => {
                            ui.colored_label(egui::Color32::from_rgb(230, 110, 90), &e.message);
                            if let Some(h) = &e.hint {
                                ui.label(egui::RichText::new(h).weak());
                            }
                            ui.label(egui::RichText::new(item.key()).weak().small());
                        }
                        Fetch::Ready(Preview::Video(v)) => {
                            ui.add(egui::Label::new(egui::RichText::new(&v.title).strong()).wrap());
                            let mut meta = v.channel.clone().unwrap_or_default();
                            if let Some(d) = v.duration {
                                meta = format!("{meta}  ·  {}", format_duration(d));
                            }
                            ui.label(egui::RichText::new(meta).weak());
                        }
                        Fetch::Ready(Preview::Playlist(p)) => {
                            ui.add(egui::Label::new(egui::RichText::new(&p.title).strong()).wrap());
                            let n_ok = p.entries.iter().filter(|e| e.available).count();
                            let mut meta = format!("Playlist  ·  {} videos", p.entries.len());
                            if n_ok < p.entries.len() {
                                meta += &format!(" ({} unavailable)", p.entries.len() - n_ok);
                            }
                            if let Some(c) = &p.channel {
                                meta = format!("{c}  ·  {meta}");
                            }
                            ui.label(egui::RichText::new(meta).weak());
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button("✖").on_hover_text("Remove").clicked() {
                        remove = Some(idx);
                    }
                });
            });

            let key = item.key();
            if !item.wants_playlist() {
                return;
            }
            if let Fetch::Ready(Preview::Playlist(p)) = &item.playlist {
                let selected = &mut item.selected;
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} of {} selected",
                        selected.iter().filter(|s| **s).count(),
                        p.entries.len()
                    ));
                    if ui.small_button("All").clicked() {
                        for (s, e) in selected.iter_mut().zip(&p.entries) {
                            *s = e.available;
                        }
                    }
                    if ui.small_button("None").clicked() {
                        selected.iter_mut().for_each(|s| *s = false);
                    }
                });
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                egui::ScrollArea::vertical()
                    .id_salt(("entries", key))
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_h, p.entries.len(), |ui, range| {
                        for i in range {
                            let e = &p.entries[i];
                            ui.horizontal(|ui| {
                                ui.add_enabled(
                                    e.available,
                                    egui::Checkbox::without_text(&mut selected[i]),
                                );
                                let dur = e.duration.map(format_duration).unwrap_or_default();
                                let text = format!("{}. {}", i + 1, e.title);
                                let label = ui.add_enabled(
                                    e.available,
                                    egui::Label::new(text)
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                );
                                if label.clicked() && e.available {
                                    selected[i] = !selected[i];
                                }
                                if !e.available {
                                    label.on_hover_text("Private or deleted; can't be downloaded");
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(egui::RichText::new(dur).weak());
                                    },
                                );
                            });
                        }
                    });
            }
        });
    }
    if let Some(idx) = fetch_playlist
        && let Link::VideoInPlaylist { list, .. } = app.input.items[idx].link.clone()
    {
        let fetch = app
            .input
            .request(&app.backend, Link::playlist_url(&list), true);
        app.input.items[idx].playlist = fetch;
    }
    if let Some(idx) = remove {
        app.input.items.remove(idx);
    }
}
