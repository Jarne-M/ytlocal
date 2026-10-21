//! Locating, verifying, downloading and updating the external tools:
//! yt-dlp (required), ffmpeg + ffprobe (required) and deno (optional; yt-dlp
//! uses it to solve YouTube's JavaScript challenges).

pub mod download;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::paths;
use crate::process;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Ffprobe,
    Deno,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe, Tool::Deno];

    pub fn exe_name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp.exe",
            Tool::Ffmpeg => "ffmpeg.exe",
            Tool::Ffprobe => "ffprobe.exe",
            Tool::Deno => "deno.exe",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::Deno => "deno",
        }
    }

    fn version_args(self) -> &'static [&'static str] {
        match self {
            Tool::YtDlp | Tool::Deno => &["--version"],
            Tool::Ffmpeg | Tool::Ffprobe => &["-version"],
        }
    }

    /// Extract a short version string from the tool's version output.
    pub fn parse_version(self, stdout: &str) -> Option<String> {
        let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
        match self {
            // "2026.08.19"
            Tool::YtDlp => Some(first.to_string()),
            // "ffmpeg version N-121234-gabcdef-20261020 Copyright (c) ..."
            Tool::Ffmpeg | Tool::Ffprobe => {
                let mut words = first.split_whitespace();
                words.find(|w| *w == "version")?;
                words.next().map(str::to_string)
            }
            // "deno 2.9.8 (stable, release, x86_64-pc-windows-msvc)"
            Tool::Deno => first.split_whitespace().nth(1).map(str::to_string),
        }
    }
}

/// Which download package provides a tool (ffmpeg and ffprobe ship together).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Package {
    YtDlp,
    Ffmpeg,
    Deno,
}

impl Package {
    pub fn label(self) -> &'static str {
        match self {
            Package::YtDlp => "yt-dlp",
            Package::Ffmpeg => "FFmpeg",
            Package::Deno => "Deno",
        }
    }

    pub fn tools(self) -> &'static [Tool] {
        match self {
            Package::YtDlp => &[Tool::YtDlp],
            Package::Ffmpeg => &[Tool::Ffmpeg, Tool::Ffprobe],
            Package::Deno => &[Tool::Deno],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolState {
    Missing,
    Ready {
        path: PathBuf,
        version: String,
    },
    /// Found but it does not run (corrupt download, wrong architecture, ...).
    Broken {
        path: PathBuf,
        error: String,
    },
}

impl ToolState {
    pub fn path(&self) -> Option<&Path> {
        match self {
            ToolState::Ready { path, .. } => Some(path),
            _ => None,
        }
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, ToolState::Ready { .. })
    }
}

/// Status of every tool, as last detected.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolReport {
    pub ytdlp: ToolState,
    pub ffmpeg: ToolState,
    pub ffprobe: ToolState,
    pub deno: ToolState,
}

impl Default for ToolReport {
    fn default() -> Self {
        Self {
            ytdlp: ToolState::Missing,
            ffmpeg: ToolState::Missing,
            ffprobe: ToolState::Missing,
            deno: ToolState::Missing,
        }
    }
}

impl ToolReport {
    pub fn get(&self, tool: Tool) -> &ToolState {
        match tool {
            Tool::YtDlp => &self.ytdlp,
            Tool::Ffmpeg => &self.ffmpeg,
            Tool::Ffprobe => &self.ffprobe,
            Tool::Deno => &self.deno,
        }
    }

    fn get_mut(&mut self, tool: Tool) -> &mut ToolState {
        match tool {
            Tool::YtDlp => &mut self.ytdlp,
            Tool::Ffmpeg => &mut self.ffmpeg,
            Tool::Ffprobe => &mut self.ffprobe,
            Tool::Deno => &mut self.deno,
        }
    }

    pub fn package_ready(&self, pkg: Package) -> bool {
        pkg.tools().iter().all(|t| self.get(*t).is_ready())
    }

    /// Everything needed to download and convert is available.
    pub fn required_ready(&self) -> bool {
        self.package_ready(Package::YtDlp) && self.package_ready(Package::Ffmpeg)
    }

    pub fn paths(&self) -> ToolPaths {
        ToolPaths {
            ytdlp: self.ytdlp.path().map(Path::to_path_buf),
            ffmpeg: self.ffmpeg.path().map(Path::to_path_buf),
            ffprobe: self.ffprobe.path().map(Path::to_path_buf),
            deno: self.deno.path().map(Path::to_path_buf),
        }
    }
}

/// Resolved paths of the working tools, handed to download jobs.
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub deno: Option<PathBuf>,
}

/// Places to look for a tool, in priority order: next to the app,
/// `%APPDATA%\YtLocal\bin`, then every directory on PATH.
pub fn candidates(tool: Tool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = paths::exe_dir() {
        dirs.push(d);
    }
    dirs.push(paths::bin_dir());
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        let p = d.join(tool.exe_name());
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Run `tool --version` and return the parsed version.
pub async fn probe(tool: Tool, path: &Path) -> Result<String, String> {
    // PyInstaller builds unpack themselves on first start and antivirus may scan
    // them, so be generous.
    let out = process::run_capture(path, tool.version_args(), Duration::from_secs(90))
        .await
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let msg = out.stderr.lines().last().unwrap_or("").trim().to_string();
        return Err(format!("exited with {} {}", out.status, msg));
    }
    tool.parse_version(&out.stdout)
        .ok_or_else(|| "unexpected --version output".to_string())
}

/// Find the first candidate that exists and actually runs.
pub async fn locate(tool: Tool) -> ToolState {
    let mut broken = None;
    for path in candidates(tool) {
        if !path.is_file() {
            continue;
        }
        match probe(tool, &path).await {
            Ok(version) => return ToolState::Ready { path, version },
            Err(error) => {
                if broken.is_none() {
                    broken = Some(ToolState::Broken { path, error });
                }
            }
        }
    }
    broken.unwrap_or(ToolState::Missing)
}

pub async fn detect_all() -> ToolReport {
    let (a, b, c, d) = tokio::join!(
        locate(Tool::YtDlp),
        locate(Tool::Ffmpeg),
        locate(Tool::Ffprobe),
        locate(Tool::Deno)
    );
    let mut report = ToolReport::default();
    for (tool, state) in Tool::ALL.into_iter().zip([a, b, c, d]) {
        *report.get_mut(tool) = state;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(
            Tool::YtDlp.parse_version("2026.08.19\n").as_deref(),
            Some("2026.08.19")
        );
        assert_eq!(
            Tool::Ffmpeg
                .parse_version("ffmpeg version N-121512-g1a2b3c-20261020 Copyright (c) 2000-2026\nbuilt with gcc")
                .as_deref(),
            Some("N-121512-g1a2b3c-20261020")
        );
        assert_eq!(
            Tool::Deno
                .parse_version("deno 2.9.8 (stable, release, x86_64-pc-windows-msvc)\nv8 15.0")
                .as_deref(),
            Some("2.9.8")
        );
        assert_eq!(Tool::Ffmpeg.parse_version("garbage"), None);
        assert_eq!(Tool::YtDlp.parse_version("  \n"), None);
    }

    #[test]
    fn candidates_start_with_app_locations() {
        let c = candidates(Tool::YtDlp);
        assert!(c.iter().any(|p| p.starts_with(paths::bin_dir())));
        assert!(c.iter().all(|p| p.ends_with("yt-dlp.exe")));
    }
}
