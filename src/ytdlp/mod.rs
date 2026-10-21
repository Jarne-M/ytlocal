//! Everything about talking to yt-dlp: links, command lines, output parsing,
//! metadata and error messages.

pub mod command;
pub mod errors;
pub mod filename;
pub mod metadata;
pub mod progress;
pub mod url;

use std::time::Duration;

use crate::binaries::ToolPaths;
use crate::process;
use errors::UserError;
use metadata::Preview;

/// Fetch title/channel/duration (or the playlist's items) without downloading.
pub async fn fetch_preview(
    tools: &ToolPaths,
    url: &str,
    playlist: bool,
) -> Result<Preview, UserError> {
    let ytdlp = tools.ytdlp.as_deref().ok_or_else(|| {
        UserError::new("yt-dlp is missing.").with_hint("Download it in the Tools panel.")
    })?;
    let args = command::preview_args(url, playlist, tools);
    let out = process::run_capture(ytdlp, &args, Duration::from_secs(180))
        .await
        .map_err(|e| {
            UserError::new(format!("Couldn't run yt-dlp: {e}"))
                .with_hint("It may be corrupt; reinstall it in the Tools panel.")
        })?;
    if !out.status.success() {
        let lines: Vec<String> = out.stderr.lines().map(str::to_string).collect();
        return Err(errors::classify(&lines));
    }
    metadata::parse(&out.stdout).map_err(UserError::new)
}
