//! ffprobe/ffmpeg steps we run ourselves: making sure an MP4 really is
//! H.264 + AAC (plays on TVs, phones, editors and Windows' own player).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::process::{self, ProcessTree};
use crate::ytdlp::errors::UserError;

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub index: usize,
    pub kind: String,
    pub codec: String,
    pub attached_pic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub format_name: String,
    pub duration: Option<f64>,
    pub streams: Vec<Stream>,
}

pub fn parse_probe(json: &str) -> Option<Probe> {
    let v: Value = serde_json::from_str(json).ok()?;
    let format = v.get("format")?;
    let streams = v
        .get("streams")?
        .as_array()?
        .iter()
        .filter_map(|s| {
            Some(Stream {
                index: s.get("index")?.as_u64()? as usize,
                kind: s.get("codec_type")?.as_str()?.to_string(),
                codec: s
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                attached_pic: s
                    .get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .and_then(Value::as_u64)
                    == Some(1),
            })
        })
        .collect();
    Some(Probe {
        format_name: format
            .get("format_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        duration: format
            .get("duration")
            .and_then(Value::as_str)
            .and_then(|d| d.parse().ok()),
        streams,
    })
}

pub async fn probe(ffprobe: &Path, file: &Path) -> Result<Probe, UserError> {
    let file_arg = file.as_os_str().to_owned();
    let out = process::run_capture(
        ffprobe,
        [
            "-v".into(),
            "error".into(),
            "-print_format".into(),
            "json".into(),
            "-show_format".into(),
            "-show_streams".into(),
            file_arg,
        ],
        Duration::from_secs(60),
    )
    .await
    .map_err(|e| UserError::new(format!("Couldn't run ffprobe: {e}")))?;
    if !out.status.success() {
        return Err(
            UserError::new("ffprobe couldn't read the downloaded file.").with_detail(out.stderr)
        );
    }
    parse_probe(&out.stdout).ok_or_else(|| UserError::new("ffprobe returned unreadable output."))
}

/// ffmpeg codec arguments to make `p` a standard MP4, or `None` if it already is.
pub fn mp4_plan(p: &Probe) -> Option<Vec<String>> {
    let main_video = p
        .streams
        .iter()
        .find(|s| s.kind == "video" && !s.attached_pic);
    let video_ok = main_video.is_none_or(|s| s.codec == "h264");
    let audio_ok = p
        .streams
        .iter()
        .filter(|s| s.kind == "audio")
        .all(|s| s.codec == "aac");
    let subs_ok = p
        .streams
        .iter()
        .filter(|s| s.kind == "subtitle")
        .all(|s| s.codec == "mov_text");
    let container_ok = p.format_name.split(',').any(|f| f == "mp4");
    if video_ok && audio_ok && subs_ok && container_ok {
        return None;
    }

    let mut a: Vec<String> = ["-map", "0:v?", "-map", "0:a?", "-map", "0:s?", "-c", "copy"]
        .map(String::from)
        .to_vec();
    if let Some(v) = main_video.filter(|_| !video_ok) {
        // Output stream specifier: position among the (mapped) video streams.
        let nth = p
            .streams
            .iter()
            .filter(|s| s.kind == "video")
            .position(|s| s.index == v.index)
            .unwrap_or(0);
        a.extend([
            format!("-c:v:{nth}"),
            "libx264".into(),
            "-preset".into(),
            "medium".into(),
            "-crf".into(),
            "20".into(),
            // 10-bit/4:4:4 sources would otherwise give High 10 profile, which many players reject.
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]);
    }
    if !audio_ok {
        a.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
    }
    if !subs_ok {
        a.extend(["-c:s", "mov_text"].map(String::from));
    }
    a.extend(
        [
            "-map_metadata",
            "0",
            "-map_chapters",
            "0",
            "-movflags",
            "+faststart",
        ]
        .map(String::from),
    );
    Some(a)
}

/// Does `mp4_plan` re-encode video (slow) rather than just audio/remux (fast)?
pub fn plan_reencodes_video(plan: &[String]) -> bool {
    plan.iter().any(|a| a == "libx264")
}

/// Run ffmpeg with `codec_args` from `input` to `output`, reporting progress
/// (0..=1) based on `duration`. Cancellable; partial output is removed.
pub async fn convert(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    codec_args: &[String],
    duration: Option<f64>,
    cancel: &CancellationToken,
    on_progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), Option<UserError>> {
    let mut cmd = process::command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(input)
        .args(codec_args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| Some(UserError::new(format!("Couldn't start ffmpeg: {e}"))))?;
    let tree = ProcessTree::attach(&child);
    let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();
    let stderr = child.stderr.take().expect("piped");
    // Collect stderr concurrently so a chatty ffmpeg never blocks on a full pipe.
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(l)) = lines.next_line().await {
            tail.push(l);
            if tail.len() > 30 {
                tail.remove(0);
            }
        }
        tail
    });

    let result = loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tree.kill();
                let _ = child.wait().await;
                break Err(None);
            }
            line = stdout.next_line() => match line {
                Ok(Some(l)) => {
                    // out_time_us / out_time_ms are both microseconds in ffmpeg.
                    if let Some(us) = l.strip_prefix("out_time_us=").or_else(|| l.strip_prefix("out_time_ms="))
                        && let (Ok(us), Some(d)) = (us.trim().parse::<f64>(), duration)
                        && d > 0.0
                    {
                        on_progress((us / 1e6 / d).clamp(0.0, 1.0) as f32);
                    }
                }
                _ => {
                    let status = child.wait().await;
                    let tail = stderr_task.await.unwrap_or_default();
                    break match status {
                        Ok(s) if s.success() => Ok(()),
                        _ => Err(Some(conversion_error(&tail))),
                    };
                }
            }
        }
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(output).await;
    }
    result
}

fn conversion_error(tail: &[String]) -> UserError {
    let text = tail.join("\n");
    let lower = text.to_lowercase();
    if lower.contains("no space left") || lower.contains("not enough space") {
        UserError::new("The disk is full.").with_detail(text)
    } else {
        UserError::new("Converting the video to MP4 (H.264/AAC) failed.").with_detail(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(format: &str, streams: &[(&str, &str, bool)]) -> Probe {
        Probe {
            format_name: format.into(),
            duration: Some(10.0),
            streams: streams
                .iter()
                .enumerate()
                .map(|(i, (k, c, pic))| Stream {
                    index: i,
                    kind: k.to_string(),
                    codec: c.to_string(),
                    attached_pic: *pic,
                })
                .collect(),
        }
    }

    const MP4: &str = "mov,mp4,m4a,3gp,3g2,mj2";

    #[test]
    fn compatible_mp4_needs_nothing() {
        assert_eq!(
            mp4_plan(&probe(
                MP4,
                &[("video", "h264", false), ("audio", "aac", false)]
            )),
            None
        );
        // Cover art as an extra mjpeg stream is fine.
        assert_eq!(
            mp4_plan(&probe(
                MP4,
                &[
                    ("video", "h264", false),
                    ("audio", "aac", false),
                    ("video", "mjpeg", true)
                ]
            )),
            None
        );
    }

    #[test]
    fn vp9_in_mp4_is_reencoded_but_cover_is_copied() {
        let plan = mp4_plan(&probe(
            MP4,
            &[
                ("video", "mjpeg", true),
                ("video", "vp9", false),
                ("audio", "aac", false),
            ],
        ))
        .unwrap();
        assert!(plan_reencodes_video(&plan));
        // The real video is the second video stream; the cover (first) stays a copy.
        let i = plan.iter().position(|a| a == "libx264").unwrap();
        assert_eq!(plan[i - 1], "-c:v:1");
        assert!(!plan.contains(&"-c:a".to_string()));

        // Audio first in the input doesn't shift the video specifier.
        let plan = mp4_plan(&probe(
            MP4,
            &[("audio", "aac", false), ("video", "vp9", false)],
        ))
        .unwrap();
        assert!(plan.windows(2).any(|w| w == ["-c:v:0", "libx264"]));
    }

    #[test]
    fn opus_audio_only_reencodes_audio() {
        let plan = mp4_plan(&probe(
            MP4,
            &[("video", "h264", false), ("audio", "opus", false)],
        ))
        .unwrap();
        assert!(!plan_reencodes_video(&plan));
        assert!(plan.windows(2).any(|w| w == ["-c:a", "aac"]));
    }

    #[test]
    fn webm_container_is_remuxed_and_subs_converted() {
        let plan = mp4_plan(&probe(
            "matroska,webm",
            &[
                ("video", "h264", false),
                ("audio", "aac", false),
                ("subtitle", "webvtt", false),
            ],
        ))
        .unwrap();
        assert!(!plan_reencodes_video(&plan));
        assert!(plan.windows(2).any(|w| w == ["-c:s", "mov_text"]));
        assert!(plan.windows(2).any(|w| w == ["-movflags", "+faststart"]));
    }

    #[test]
    fn parses_ffprobe_json() {
        let json = r#"{"streams": [
            {"index": 0, "codec_name": "av1", "codec_type": "video", "disposition": {"attached_pic": 0}},
            {"index": 1, "codec_name": "opus", "codec_type": "audio", "disposition": {"attached_pic": 0}},
            {"index": 2, "codec_name": "mjpeg", "codec_type": "video", "disposition": {"attached_pic": 1}}
        ], "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "84.034000"}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.duration, Some(84.034));
        assert_eq!(p.streams.len(), 3);
        assert!(p.streams[2].attached_pic);
        let plan = mp4_plan(&p).unwrap();
        assert!(plan.windows(2).any(|w| w == ["-c:v:0", "libx264"]));
    }
}
