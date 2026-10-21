//! Thumbnail cache: requested once from the backend, shown via egui's image
//! loaders from in-memory bytes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::{Backend, Request};

enum Thumb {
    Loading,
    Ready(Arc<[u8]>),
    Failed,
}

#[derive(Default)]
pub struct Thumbs {
    cache: HashMap<String, Thumb>,
    wanted: Vec<String>,
}

impl Thumbs {
    pub fn loaded(&mut self, url: String, bytes: Option<Arc<[u8]>>) {
        let t = bytes.map_or(Thumb::Failed, Thumb::Ready);
        self.cache.insert(url, t);
    }

    /// Ask the backend for thumbnails first seen this frame.
    pub fn request_missing(&mut self, backend: &Backend) {
        for url in self.wanted.drain(..) {
            backend.send(Request::Thumbnail(url));
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, url: Option<&str>, size: egui::Vec2) {
        let bytes = url.and_then(|u| match self.cache.get(u) {
            Some(Thumb::Ready(b)) => Some(b.clone()),
            Some(_) => None,
            None => {
                self.cache.insert(u.to_string(), Thumb::Loading);
                self.wanted.push(u.to_string());
                None
            }
        });
        match (url, bytes) {
            (Some(u), Some(b)) => {
                ui.add(
                    egui::Image::from_bytes(format!("bytes://{u}"), egui::load::Bytes::Shared(b))
                        .fit_to_exact_size(size)
                        .corner_radius(4.0),
                );
            }
            _ => {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            }
        }
    }
}
