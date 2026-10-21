//! What a download job is and the states it goes through.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ytdlp::command::{Options, OutputFormat};
use crate::ytdlp::errors::UserError;

pub type JobId = u64;

/// What to do when the destination file already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictPolicy {
    /// Keep both: "name (1).mp3".
    #[default]
    Rename,
    /// Leave the existing file alone and skip the download.
    Skip,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: JobId,
    pub url: String,
    pub video_id: Option<String>,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    /// Playlist name, used as the album tag.
    pub album: Option<String>,
    pub format: OutputFormat,
    pub options: Options,
    pub output_dir: PathBuf,
    pub template: String,
    pub on_conflict: ConflictPolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    /// Starting yt-dlp / fetching video info.
    Starting,
    Downloading,
    /// Post-processing; the text says what (e.g. "Converting to MP3").
    Converting(String),
    Done(PathBuf),
    /// Destination already existed and the policy is Skip.
    Skipped(PathBuf),
    Failed(UserError),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            JobState::Done(_) | JobState::Skipped(_) | JobState::Failed(_) | JobState::Cancelled
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobState::Starting | JobState::Downloading | JobState::Converting(_)
        )
    }

    pub fn label(&self) -> &str {
        match self {
            JobState::Queued => "Queued",
            JobState::Starting => "Starting",
            JobState::Downloading => "Downloading",
            JobState::Converting(what) => what,
            JobState::Done(_) => "Done",
            JobState::Skipped(_) => "Skipped (file exists)",
            JobState::Failed(_) => "Failed",
            JobState::Cancelled => "Cancelled",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    State(JobState),
    Progress {
        fraction: Option<f32>,
        /// Bytes per second.
        speed: Option<f64>,
        /// Seconds remaining.
        eta: Option<u64>,
    },
}
