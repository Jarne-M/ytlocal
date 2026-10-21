//! Settings persisted in `%APPDATA%\YtLocal\config.json`, and the download
//! history next to it. Unknown or missing fields fall back to defaults, and a
//! corrupt file is set aside instead of crashing the app.

pub mod history;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::queue::DEFAULT_CONCURRENCY;
use crate::queue::job::ConflictPolicy;
use crate::ytdlp::command::{AudioFormat, Options, OutputFormat, VideoQuality};

pub const DEFAULT_TEMPLATE: &str = "%(title)s.%(ext)s";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Audio,
    Video,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio_dir: PathBuf,
    pub video_dir: PathBuf,
    pub mode: Mode,
    pub audio_format: AudioFormat,
    pub mp3_kbps: u32,
    pub video_quality: VideoQuality,
    /// Per-download options; `trim` is never persisted.
    pub options: Options,
    pub template: String,
    pub on_conflict: ConflictPolicy,
    pub concurrency: usize,
    pub window_size: Option<[f32; 2]>,
    /// Look for a YouTube link in the clipboard when the window gains focus.
    pub clipboard_watch: bool,
    pub notifications: bool,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            audio_dir: dirs::audio_dir().unwrap_or_else(|| home.join("Music")),
            video_dir: dirs::video_dir().unwrap_or_else(|| home.join("Videos")),
            mode: Mode::Audio,
            audio_format: AudioFormat::Mp3,
            mp3_kbps: 192,
            video_quality: VideoQuality::P1080,
            options: Options::default(),
            template: DEFAULT_TEMPLATE.into(),
            on_conflict: ConflictPolicy::Rename,
            concurrency: DEFAULT_CONCURRENCY,
            window_size: None,
            clipboard_watch: false,
            notifications: true,
        }
    }
}

impl Config {
    pub fn output_format(&self) -> OutputFormat {
        match self.mode {
            Mode::Audio => OutputFormat::Audio {
                format: self.audio_format,
                kbps: self.mp3_kbps,
            },
            Mode::Video => OutputFormat::Video {
                quality: self.video_quality,
            },
        }
    }

    pub fn output_dir(&self) -> &Path {
        match self.mode {
            Mode::Audio => &self.audio_dir,
            Mode::Video => &self.video_dir,
        }
    }

    /// Clamp values a hand-edited file might get wrong.
    fn normalized(mut self) -> Self {
        self.options.trim = None;
        self.template = normalize_template(&self.template);
        self.concurrency = self.concurrency.clamp(1, crate::queue::MAX_CONCURRENCY);
        if !crate::ytdlp::command::MP3_BITRATES.contains(&self.mp3_kbps) {
            self.mp3_kbps = 192;
        }
        self
    }

    pub fn load() -> Self {
        Self::load_from(&paths::config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(c) => c.normalized(),
            Err(_) => {
                // Keep the broken file for inspection; start fresh.
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                Self::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        write_json_atomic(&paths::config_path(), self)
    }
}

/// Make sure a filename template is usable: non-empty, relative, ends with
/// the extension field.
pub fn normalize_template(t: &str) -> String {
    let t = t.trim().trim_start_matches(['/', '\\']);
    // Drive letters / absolute paths: the output folder is chosen separately.
    let t = if t.len() >= 2 && t.as_bytes()[1] == b':' {
        &t[2..]
    } else {
        t
    };
    let t = t.trim_start_matches(['/', '\\']).trim();
    if t.is_empty() || !t.contains("%(") {
        return DEFAULT_TEMPLATE.into();
    }
    if t.contains("%(ext)s") {
        t.to_string()
    } else {
        format!("{t}.%(ext)s")
    }
}

/// Write JSON via a temp file + rename, so a crash never leaves half a file.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        assert_eq!(normalize_template(""), DEFAULT_TEMPLATE);
        assert_eq!(normalize_template("no fields"), DEFAULT_TEMPLATE);
        assert_eq!(normalize_template("%(title)s"), "%(title)s.%(ext)s");
        assert_eq!(
            normalize_template(" %(uploader)s/%(title)s.%(ext)s "),
            "%(uploader)s/%(title)s.%(ext)s"
        );
        assert_eq!(
            normalize_template("C:\\x\\%(title)s.%(ext)s"),
            "x\\%(title)s.%(ext)s"
        );
        assert_eq!(
            normalize_template("/%(title)s.%(ext)s"),
            "%(title)s.%(ext)s"
        );
    }

    #[test]
    fn round_trip_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("ytlocal-cfg-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut c = Config {
            mode: Mode::Video,
            concurrency: 3,
            ..Config::default()
        };
        c.options.trim = Some(crate::ytdlp::command::Trim {
            start: Some(1.0),
            end: None,
        });
        write_json_atomic(&path, &c).unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded.mode, Mode::Video);
        assert_eq!(loaded.concurrency, 3);
        assert_eq!(loaded.options.trim, None); // never persisted

        // Missing fields use defaults; out-of-range values are clamped.
        std::fs::write(
            &path,
            r#"{"concurrency": 99, "mp3_kbps": 7, "some_future_field": 1}"#,
        )
        .unwrap();
        let loaded = Config::load_from(&path);
        assert_eq!(loaded.concurrency, crate::queue::MAX_CONCURRENCY);
        assert_eq!(loaded.mp3_kbps, 192);
        assert_eq!(loaded.template, DEFAULT_TEMPLATE);

        // Corrupt file: defaults, and the bad file is kept aside.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Config::load_from(&path), Config::default());
        assert!(dir.join("config.json.bad").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
