//! YtLocal: a local YouTube to MP3/MP4 converter that drives yt-dlp and ffmpeg.

pub mod backend;
pub mod binaries;
pub mod config;
pub mod ffmpeg;
pub mod notify;
pub mod paths;
pub mod process;
pub mod queue;
pub mod ui;
pub mod ytdlp;
