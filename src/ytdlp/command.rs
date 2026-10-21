//! Building yt-dlp command lines. Pure functions, so they're easy to test.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::progress;
use crate::binaries::ToolPaths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioFormat {
    Mp3,
    M4a,
    Opus,
    Wav,
    Flac,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 5] = [
        AudioFormat::Mp3,
        AudioFormat::M4a,
        AudioFormat::Opus,
        AudioFormat::Wav,
        AudioFormat::Flac,
    ];

    /// yt-dlp `--audio-format` value, which is also the file extension.
    pub fn ext(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "m4a",
            AudioFormat::Opus => "opus",
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "MP3",
            AudioFormat::M4a => "M4A (AAC)",
            AudioFormat::Opus => "Opus",
            AudioFormat::Wav => "WAV",
            AudioFormat::Flac => "FLAC",
        }
    }

    /// Whether the user picks a bitrate (only MP3 is always re-encoded at a fixed rate).
    pub fn has_bitrate(self) -> bool {
        self == AudioFormat::Mp3
    }

    /// Cover art can be embedded in everything except WAV.
    pub fn supports_cover(self) -> bool {
        self != AudioFormat::Wav
    }
}

pub const MP3_BITRATES: [u32; 4] = [128, 192, 256, 320];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoQuality {
    Best,
    P1080,
    P720,
    P480,
    P360,
}

impl VideoQuality {
    pub const ALL: [VideoQuality; 5] = [
        VideoQuality::Best,
        VideoQuality::P1080,
        VideoQuality::P720,
        VideoQuality::P480,
        VideoQuality::P360,
    ];

    pub fn height(self) -> Option<u32> {
        match self {
            VideoQuality::Best => None,
            VideoQuality::P1080 => Some(1080),
            VideoQuality::P720 => Some(720),
            VideoQuality::P480 => Some(480),
            VideoQuality::P360 => Some(360),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::Best => "Best",
            VideoQuality::P1080 => "1080p",
            VideoQuality::P720 => "720p",
            VideoQuality::P480 => "480p",
            VideoQuality::P360 => "360p",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputFormat {
    Audio { format: AudioFormat, kbps: u32 },
    Video { quality: VideoQuality },
}

impl OutputFormat {
    pub fn is_audio(&self) -> bool {
        matches!(self, OutputFormat::Audio { .. })
    }

    /// Extension of the finished file.
    pub fn ext(&self) -> &'static str {
        match self {
            OutputFormat::Audio { format, .. } => format.ext(),
            OutputFormat::Video { .. } => "mp4",
        }
    }

    pub fn label(&self) -> String {
        match self {
            OutputFormat::Audio { format, kbps } if format.has_bitrate() => {
                format!("{} {kbps}k", format.label())
            }
            OutputFormat::Audio { format, .. } => format.label().to_string(),
            OutputFormat::Video { quality } => format!("MP4 {}", quality.label()),
        }
    }
}

/// Start/end in seconds; either may be open.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trim {
    pub start: Option<f64>,
    pub end: Option<f64>,
}

impl Trim {
    /// yt-dlp `--download-sections` value, e.g. "*12.5-90" or "*0-inf".
    pub fn section(&self) -> String {
        let start = self.start.unwrap_or(0.0);
        let end = self.end.map_or("inf".to_string(), fmt_secs);
        format!("*{}-{end}", fmt_secs(start))
    }
}

fn fmt_secs(s: f64) -> String {
    let s = format!("{s:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Parse "90", "1:30", "01:02:03", "1:30.5" into seconds.
pub fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let v: f64 = if last {
            p.parse().ok()?
        } else {
            p.parse::<u32>().ok()? as f64
        };
        if v < 0.0 || !v.is_finite() || (i > 0 && v >= 60.0) {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// Parse user-entered trim fields; empty fields mean "from start"/"to end".
pub fn parse_trim(start: &str, end: &str) -> Result<Option<Trim>, String> {
    let s = match start.trim() {
        "" => None,
        t => Some(parse_time(t).ok_or_else(|| format!("Invalid start time \"{t}\" (use m:ss)"))?),
    };
    let e = match end.trim() {
        "" => None,
        t => Some(parse_time(t).ok_or_else(|| format!("Invalid end time \"{t}\" (use m:ss)"))?),
    };
    if let (Some(s), Some(e)) = (s, e)
        && e <= s
    {
        return Err("End time must be after start time".into());
    }
    Ok(match (s, e) {
        (None | Some(0.0), None) => None,
        _ => Some(Trim { start: s, end: e }),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Title/artist/album tags, plus cover art for audio.
    pub embed_metadata: bool,
    pub normalize: bool,
    pub subtitles: bool,
    /// yt-dlp `--sub-langs` value, e.g. "en.*,nl".
    pub subtitle_langs: String,
    pub auto_subtitles: bool,
    pub trim: Option<Trim>,
    /// KiB/s.
    pub rate_limit: Option<u32>,
    /// For "Best" video: prefer YouTube's native H.264 streams (no slow re-encode,
    /// usually tops out at 1080p) over 1440p/4K VP9/AV1.
    pub prefer_h264: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            embed_metadata: true,
            normalize: false,
            subtitles: false,
            subtitle_langs: "en.*".into(),
            auto_subtitles: false,
            trim: None,
            rate_limit: None,
            prefer_h264: true,
        }
    }
}

/// Everything needed to build one download command.
pub struct DownloadArgs<'a> {
    pub url: &'a str,
    pub format: OutputFormat,
    pub options: &'a Options,
    /// Playlist name, written as the album tag.
    pub album: Option<&'a str>,
    pub template: &'a str,
    pub work_dir: &'a Path,
    pub tools: &'a ToolPaths,
}

const LOUDNORM: &str = "loudnorm=I=-16:TP=-1.5:LRA=11,aresample=48000";

/// Flags shared by every yt-dlp invocation.
fn base_args(tools: &ToolPaths) -> Vec<String> {
    let mut a: Vec<String> = [
        "--ignore-config",
        "--encoding",
        "utf-8",
        "--color",
        "no_color",
        "--newline",
        "--no-playlist",
    ]
    .map(String::from)
    .to_vec();
    if let Some(dir) = tools.ffmpeg.as_deref().and_then(Path::parent) {
        a.push("--ffmpeg-location".into());
        a.push(dir.display().to_string());
    }
    if let Some(deno) = &tools.deno {
        a.push("--js-runtimes".into());
        a.push(format!("deno:{}", deno.display()));
    }
    a
}

/// Quote a value for yt-dlp's shlex-split `--postprocessor-args`.
fn shlex_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Video format selection. With `prefer_h264`, take H.264 when it exists at
/// all (Best) or tie-break on it (fixed heights), so most downloads need no
/// re-encode. Resolution limits fall back to the nearest lower one.
fn video_selection(quality: VideoQuality, prefer_h264: bool) -> (String, String) {
    match quality.height() {
        None if prefer_h264 => (
            "bv*[vcodec^=avc1]+ba[acodec^=mp4a]/bv*[vcodec^=avc1]+ba/bv*+ba/b".into(),
            "res,fps,vcodec:h264,acodec:aac".into(),
        ),
        None => ("bv*+ba/b".into(), "res,fps,vcodec:h264,acodec:aac".into()),
        Some(h) => (
            "bv*+ba/b".into(),
            format!("res:{h},fps,vcodec:h264,acodec:aac"),
        ),
    }
}

pub fn download_args(d: &DownloadArgs) -> Vec<String> {
    let o = d.options;
    let mut a = base_args(d.tools);
    a.extend(
        [
            "--no-mtime",
            "--progress",
            "--no-simulate",
            "--progress-template",
            progress::DOWNLOAD_TEMPLATE,
            "--progress-template",
            progress::POSTPROCESS_TEMPLATE,
            "--print",
            progress::PRINT_FORMATS,
            "--print",
            progress::PRINT_NAME,
            "--print",
            progress::PRINT_FILE,
            "--trim-filenames",
            "160",
            "--windows-filenames",
        ]
        .map(String::from),
    );
    a.push("-P".into());
    a.push(d.work_dir.display().to_string());
    a.push("-o".into());
    a.push(d.template.to_string());

    if let Some(kib) = o.rate_limit.filter(|k| *k > 0) {
        a.push("--limit-rate".into());
        a.push(format!("{kib}K"));
    }
    if let Some(trim) = &o.trim {
        a.push("--download-sections".into());
        a.push(trim.section());
        a.push("--force-keyframes-at-cuts".into());
    }

    let mut ppa: Vec<String> = Vec::new();
    match d.format {
        OutputFormat::Audio { format, kbps } => {
            // ExtractAudio stream-copies when the source already has the target
            // codec, and filters can't be applied to a copy. When normalizing
            // M4A/Opus, prefer a source in the *other* codec so it's re-encoded.
            let selector = match (format, o.normalize) {
                (AudioFormat::M4a, true) => "ba[acodec!^=mp4a]/ba/b",
                (AudioFormat::Opus, true) => "ba[acodec!=opus]/ba/b",
                (AudioFormat::M4a, false) => "ba[acodec^=mp4a]/ba/b",
                (AudioFormat::Opus, false) => "ba[acodec=opus]/ba/b",
                _ => "ba/b",
            };
            a.extend(["-f".into(), selector.into(), "-x".into()]);
            a.extend(["--audio-format".into(), format.ext().into()]);
            if format.has_bitrate() {
                a.extend(["--audio-quality".into(), format!("{kbps}K")]);
            } else {
                a.extend(["--audio-quality".into(), "0".into()]);
            }
            if o.normalize {
                ppa.push(format!("ExtractAudio:-af {LOUDNORM}"));
            }
            if o.embed_metadata {
                a.push("--embed-metadata".into());
                if format.supports_cover() {
                    a.extend([
                        "--embed-thumbnail".into(),
                        "--convert-thumbnails".into(),
                        "jpg".into(),
                    ]);
                }
            }
        }
        OutputFormat::Video { quality } => {
            let (f, s) = video_selection(quality, o.prefer_h264);
            a.extend(["-f".into(), f, "-S".into(), s]);
            a.extend(["--merge-output-format".into(), "mp4".into()]);
            if o.embed_metadata {
                a.extend([
                    "--embed-metadata".into(),
                    "--embed-thumbnail".into(),
                    "--convert-thumbnails".into(),
                    "jpg".into(),
                ]);
            }
            if o.subtitles {
                a.push("--write-subs".into());
                if o.auto_subtitles {
                    a.push("--write-auto-subs".into());
                }
                let langs = if o.subtitle_langs.trim().is_empty() {
                    "en.*"
                } else {
                    o.subtitle_langs.trim()
                };
                a.extend(["--sub-langs".into(), langs.into(), "--embed-subs".into()]);
            }
        }
    }
    if o.embed_metadata
        && let Some(album) = d.album.filter(|s| !s.trim().is_empty())
    {
        ppa.push(format!(
            "Metadata:-metadata {}",
            shlex_quote(&format!("album={album}"))
        ));
    }
    for p in ppa {
        a.push("--postprocessor-args".into());
        a.push(p);
    }

    a.push("--".into());
    a.push(d.url.to_string());
    a
}

/// Metadata-only run: `-J` dumps one JSON document. Playlists are listed
/// flat (no per-video requests), which is fast even for long playlists.
pub fn preview_args(url: &str, playlist: bool, tools: &ToolPaths) -> Vec<String> {
    let mut a = base_args(tools);
    if playlist {
        // base_args says --no-playlist; the later flag wins.
        a.extend(["--yes-playlist".into(), "--flat-playlist".into()]);
    }
    a.extend(["-J".into(), "--".into(), url.to_string()]);
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tools() -> ToolPaths {
        ToolPaths {
            ytdlp: Some(PathBuf::from(r"C:\bin\yt-dlp.exe")),
            ffmpeg: Some(PathBuf::from(r"C:\bin\ffmpeg.exe")),
            ffprobe: Some(PathBuf::from(r"C:\bin\ffprobe.exe")),
            deno: Some(PathBuf::from(r"C:\bin\deno.exe")),
        }
    }

    fn build(format: OutputFormat, options: &Options, album: Option<&str>) -> Vec<String> {
        let t = tools();
        download_args(&DownloadArgs {
            url: "https://www.youtube.com/watch?v=dPZTh2NKTm4",
            format,
            options,
            album,
            template: "%(title)s.%(ext)s",
            work_dir: Path::new(r"C:\tmp\job-1"),
            tools: &t,
        })
    }

    /// Value following `flag` (first occurrence).
    fn value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    fn has(args: &[String], flag: &str) -> bool {
        args.iter().any(|a| a == flag)
    }

    #[test]
    fn mp3_command() {
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 320,
            },
            &Options::default(),
            None,
        );
        assert_eq!(value(&args, "--audio-format"), Some("mp3"));
        assert_eq!(value(&args, "--audio-quality"), Some("320K"));
        assert!(has(&args, "-x"));
        assert!(has(&args, "--embed-thumbnail"));
        assert!(has(&args, "--embed-metadata"));
        assert_eq!(value(&args, "--ffmpeg-location"), Some(r"C:\bin"));
        assert_eq!(value(&args, "--js-runtimes"), Some(r"deno:C:\bin\deno.exe"));
        assert_eq!(value(&args, "-P"), Some(r"C:\tmp\job-1"));
        assert_eq!(value(&args, "-o"), Some("%(title)s.%(ext)s"));
        // URL last, after "--" so a URL starting with '-' can't be read as a flag.
        assert_eq!(
            &args[args.len() - 2..],
            ["--", "https://www.youtube.com/watch?v=dPZTh2NKTm4"]
        );
        assert!(!has(&args, "--limit-rate"));
        assert!(!has(&args, "--download-sections"));
        assert!(!has(&args, "--postprocessor-args"));
    }

    #[test]
    fn wav_has_no_cover_and_lossless_quality() {
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Wav,
                kbps: 320,
            },
            &Options::default(),
            None,
        );
        assert_eq!(value(&args, "--audio-format"), Some("wav"));
        assert_eq!(value(&args, "--audio-quality"), Some("0"));
        assert!(!has(&args, "--embed-thumbnail"));
        assert!(has(&args, "--embed-metadata"));
    }

    #[test]
    fn normalize_forces_reencode_for_copyable_codecs() {
        let o = Options {
            normalize: true,
            ..Options::default()
        };
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Opus,
                kbps: 0,
            },
            &o,
            None,
        );
        assert_eq!(value(&args, "-f"), Some("ba[acodec!=opus]/ba/b"));
        assert!(
            value(&args, "--postprocessor-args")
                .unwrap()
                .starts_with("ExtractAudio:-af loudnorm")
        );
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::M4a,
                kbps: 0,
            },
            &o,
            None,
        );
        assert_eq!(value(&args, "-f"), Some("ba[acodec!^=mp4a]/ba/b"));
    }

    #[test]
    fn album_is_quoted_for_shlex() {
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            &Options::default(),
            Some(r#"My "Best" Mix: Vol\1"#),
        );
        assert_eq!(
            value(&args, "--postprocessor-args"),
            Some(r#"Metadata:-metadata "album=My \"Best\" Mix: Vol\\1""#)
        );
        // No metadata embedding: no album either.
        let o = Options {
            embed_metadata: false,
            ..Options::default()
        };
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            &o,
            Some("x"),
        );
        assert!(!has(&args, "--postprocessor-args"));
        assert!(!has(&args, "--embed-metadata"));
    }

    #[test]
    fn video_commands() {
        let args = build(
            OutputFormat::Video {
                quality: VideoQuality::P720,
            },
            &Options::default(),
            None,
        );
        assert_eq!(value(&args, "-f"), Some("bv*+ba/b"));
        assert_eq!(
            value(&args, "-S"),
            Some("res:720,fps,vcodec:h264,acodec:aac")
        );
        assert_eq!(value(&args, "--merge-output-format"), Some("mp4"));
        assert!(!has(&args, "-x"));
        assert!(!has(&args, "--write-subs"));

        let args = build(
            OutputFormat::Video {
                quality: VideoQuality::Best,
            },
            &Options::default(),
            None,
        );
        assert!(value(&args, "-f").unwrap().starts_with("bv*[vcodec^=avc1]"));
        let o = Options {
            prefer_h264: false,
            ..Options::default()
        };
        let args = build(
            OutputFormat::Video {
                quality: VideoQuality::Best,
            },
            &o,
            None,
        );
        assert_eq!(value(&args, "-f"), Some("bv*+ba/b"));
    }

    #[test]
    fn subtitles_trim_and_rate_limit() {
        let o = Options {
            subtitles: true,
            subtitle_langs: "en.*,nl".into(),
            trim: Some(Trim {
                start: Some(12.5),
                end: Some(90.0),
            }),
            rate_limit: Some(500),
            ..Options::default()
        };
        let args = build(
            OutputFormat::Video {
                quality: VideoQuality::P1080,
            },
            &o,
            None,
        );
        assert_eq!(value(&args, "--sub-langs"), Some("en.*,nl"));
        assert!(has(&args, "--embed-subs"));
        assert!(!has(&args, "--write-auto-subs"));
        assert_eq!(value(&args, "--download-sections"), Some("*12.5-90"));
        assert!(has(&args, "--force-keyframes-at-cuts"));
        assert_eq!(value(&args, "--limit-rate"), Some("500K"));

        // Subtitles are a video-only option.
        let args = build(
            OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            &o,
            None,
        );
        assert!(!has(&args, "--write-subs"));
    }

    #[test]
    fn missing_tools_are_omitted() {
        let t = ToolPaths::default();
        let a = preview_args("https://youtu.be/x", true, &t);
        assert!(!has(&a, "--ffmpeg-location"));
        assert!(!has(&a, "--js-runtimes"));
        assert!(has(&a, "--flat-playlist"));
        assert!(has(&a, "-J"));
    }

    #[test]
    fn time_parsing() {
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time("1:30"), Some(90.0));
        assert_eq!(parse_time("01:02:03"), Some(3723.0));
        assert_eq!(parse_time("1:30.5"), Some(90.5));
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn trim_parsing() {
        assert_eq!(parse_trim("", ""), Ok(None));
        assert_eq!(parse_trim("0", ""), Ok(None));
        assert_eq!(
            parse_trim("0:10", "0:20"),
            Ok(Some(Trim {
                start: Some(10.0),
                end: Some(20.0)
            }))
        );
        assert_eq!(
            parse_trim("", "1:00"),
            Ok(Some(Trim {
                start: None,
                end: Some(60.0)
            }))
        );
        assert!(parse_trim("0:20", "0:10").is_err());
        assert!(parse_trim("x", "").is_err());
        assert_eq!(
            Trim {
                start: None,
                end: None
            }
            .section(),
            "*0-inf"
        );
        assert_eq!(
            Trim {
                start: Some(1.25),
                end: None
            }
            .section(),
            "*1.25-inf"
        );
    }
}
