//! The last finished downloads, persisted in `history.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

pub const MAX_ENTRIES: usize = 50;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
    pub path: PathBuf,
    /// e.g. "MP3 192k".
    pub format: String,
    /// Unix seconds.
    pub finished_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.retain(|e| e.path != entry.path);
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
    }

    pub fn load() -> Self {
        Self::load_from(&paths::history_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<History>(&t).ok())
            .map(|mut h| {
                h.entries.truncate(MAX_ENTRIES);
                h
            })
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        super::write_json_atomic(&paths::history_path(), self)
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: usize) -> HistoryEntry {
        HistoryEntry {
            title: format!("t{i}"),
            url: String::new(),
            path: PathBuf::from(format!("C:\\f{i}.mp3")),
            format: "MP3".into(),
            finished_at: i as u64,
        }
    }

    #[test]
    fn keeps_newest_fifty_without_duplicates() {
        let mut h = History::default();
        for i in 0..60 {
            h.push(entry(i));
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.entries[0].title, "t59");
        h.push(entry(30));
        assert_eq!(h.entries[0].title, "t30");
        assert_eq!(h.entries.iter().filter(|e| e.title == "t30").count(), 1);
    }

    #[test]
    fn corrupt_history_is_empty() {
        let dir = std::env::temp_dir().join(format!("ytlocal-hist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("history.json");
        std::fs::write(&p, "garbage").unwrap();
        assert!(History::load_from(&p).entries.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
