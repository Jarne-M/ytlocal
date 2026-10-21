//! Runs one job: yt-dlp into a private work folder, optional MP4 fix-up with
//! ffmpeg, then a safe move into the output folder. The work folder is
//! always deleted afterwards, so cancelled or failed jobs leave nothing behind.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::job::{ConflictPolicy, JobEvent, JobSpec, JobState};
use crate::binaries::ToolPaths;
use crate::ffmpeg;
use crate::paths;
use crate::process::{self, ProcessTree};
use crate::ytdlp::command::{DownloadArgs, OutputFormat, download_args};
use crate::ytdlp::errors::{UserError, classify};
use crate::ytdlp::filename;
use crate::ytdlp::progress::{
    DownloadStatus, Line, OverallProgress, PostprocessStatus, parse_line,
};

pub type Report<'a> = &'a (dyn Fn(JobEvent) + Send + Sync);

const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const STDERR_KEEP: usize = 200;

/// Read one line, tolerating invalid UTF-8. Cancel-safe: partial data stays in `buf`.
async fn next_line<R: AsyncBufRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Option<String> {
    let n = r.read_until(b'\n', buf).await.ok()?;
    if n == 0 && buf.is_empty() {
        return None;
    }
    let line = String::from_utf8_lossy(buf)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    buf.clear();
    Some(line)
}

/// Where a file produced in `work` should end up, with extension `ext`.
pub fn destination(spec: &JobSpec, work: &Path, produced: &Path, ext: &str) -> PathBuf {
    let rel = produced
        .strip_prefix(work)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| PathBuf::from(produced.file_name().unwrap_or_default()));
    let rel = filename::sanitize_relative(&rel.with_extension(ext));
    filename::fit_in(&spec.output_dir, &rel)
}

/// Serializes picking a free name so two jobs can't claim the same one.
static NAME_LOCK: Mutex<()> = Mutex::new(());

/// Move `src` to `dest` (or a free variant of it). Works across drives.
fn move_into_place(src: &Path, dest: &Path, policy: ConflictPolicy) -> Result<JobState, UserError> {
    let io_err = |what: &str, e: std::io::Error| {
        let disk_full = matches!(e.raw_os_error(), Some(112) | Some(39));
        if disk_full {
            UserError::new("The disk is full.").with_detail(e.to_string())
        } else {
            UserError::new(format!("{what}: {e}"))
        }
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| io_err("Couldn't create the output folder", e))?;
    }
    let target = {
        let _guard = NAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if dest.exists() && policy == ConflictPolicy::Skip {
            return Ok(JobState::Skipped(dest.to_path_buf()));
        }
        let target = filename::unique_path(dest);
        // Reserve the name; the real file replaces this placeholder below.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| io_err("Couldn't write to the output folder", e))?;
        target
    };
    if std::fs::rename(src, &target).is_err() {
        // Different drive: copy, then remove the original.
        if let Err(e) = std::fs::copy(src, &target) {
            let _ = std::fs::remove_file(&target);
            return Err(io_err("Couldn't copy the file to the output folder", e));
        }
        let _ = std::fs::remove_file(src);
    }
    Ok(JobState::Done(target))
}

async fn remove_dir_with_retry(dir: &Path) {
    // Killed processes can hold file handles for a moment.
    for attempt in 1..=10 {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => tokio::time::sleep(Duration::from_millis(150 * attempt)).await,
        }
    }
}

/// Remove work folders left behind by a crash (older than a few hours, so a
/// second running instance is never disturbed).
pub async fn clean_stale_work_dirs() {
    let Ok(mut rd) = tokio::fs::read_dir(paths::temp_root()).await else {
        return;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let stale = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(6 * 3600));
        if stale {
            remove_dir_with_retry(&entry.path()).await;
        }
    }
}

fn postprocess_label(name: &str, format: &OutputFormat) -> String {
    match name {
        "ExtractAudio" => format!("Converting to {}", format.ext().to_uppercase()),
        "Merger" => "Merging video and audio".into(),
        "EmbedThumbnail" => "Embedding cover art".into(),
        "Metadata" => "Writing tags".into(),
        "EmbedSubtitle" => "Embedding subtitles".into(),
        "VideoConvertor" | "VideoRemuxer" => "Converting video".into(),
        "MoveFiles" => "Finishing".into(),
        _ => "Processing".into(),
    }
}

pub async fn run(
    spec: &JobSpec,
    tools: &ToolPaths,
    cancel: &CancellationToken,
    report: Report<'_>,
) -> JobState {
    let Some(ytdlp) = tools.ytdlp.as_deref() else {
        return JobState::Failed(
            UserError::new("yt-dlp is missing.").with_hint("Download it in the Tools panel."),
        );
    };
    if tools.ffmpeg.is_none() || tools.ffprobe.is_none() {
        return JobState::Failed(
            UserError::new("ffmpeg is missing.").with_hint("Download it in the Tools panel."),
        );
    }
    if let Err(e) = std::fs::create_dir_all(&spec.output_dir) {
        return JobState::Failed(UserError::new(format!(
            "Can't use output folder {}: {e}",
            spec.output_dir.display()
        )));
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let work = paths::temp_root().join(format!("job-{}-{nonce}", spec.id));
    if let Err(e) = tokio::fs::create_dir_all(&work).await {
        return JobState::Failed(UserError::new(format!(
            "Can't create a temporary folder: {e}"
        )));
    }

    let state = run_in(spec, tools, ytdlp, &work, cancel, report).await;
    remove_dir_with_retry(&work).await;
    state
}

struct Progress<'a> {
    report: Report<'a>,
    state: JobState,
    last_sent: Instant,
}

impl Progress<'_> {
    fn set_state(&mut self, s: JobState) {
        if self.state != s {
            self.state = s.clone();
            (self.report)(JobEvent::State(s));
        }
    }

    fn progress(
        &mut self,
        fraction: Option<f32>,
        speed: Option<f64>,
        eta: Option<u64>,
        force: bool,
    ) {
        if force || self.last_sent.elapsed() >= PROGRESS_INTERVAL {
            self.last_sent = Instant::now();
            (self.report)(JobEvent::Progress {
                fraction,
                speed,
                eta,
            });
        }
    }
}

async fn run_in(
    spec: &JobSpec,
    tools: &ToolPaths,
    ytdlp: &Path,
    work: &Path,
    cancel: &CancellationToken,
    report: Report<'_>,
) -> JobState {
    let mut p = Progress {
        report,
        state: JobState::Queued,
        last_sent: Instant::now() - PROGRESS_INTERVAL,
    };
    p.set_state(JobState::Starting);

    let args = download_args(&DownloadArgs {
        url: &spec.url,
        format: spec.format,
        options: &spec.options,
        album: spec.album.as_deref(),
        template: &spec.template,
        work_dir: work,
        tools,
    });
    let mut child = match process::command(ytdlp)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return JobState::Failed(
                UserError::new(format!("Couldn't start yt-dlp: {e}"))
                    .with_hint("It may be corrupt; reinstall it in the Tools panel."),
            );
        }
    };
    let tree = ProcessTree::attach(&child);
    let mut out = BufReader::new(child.stdout.take().expect("piped"));
    let mut err = BufReader::new(child.stderr.take().expect("piped"));
    let (mut obuf, mut ebuf) = (Vec::new(), Vec::new());
    let (mut out_done, mut err_done) = (false, false);

    let mut stderr_tail: VecDeque<String> = VecDeque::new();
    let mut overall = OverallProgress::default();
    let mut downloaded_any = false;
    let mut final_file: Option<PathBuf> = None;
    let final_ext = spec.format.ext();

    while !(out_done && err_done) {
        // yt-dlp writes some of our YTL* lines to stderr (e.g. postprocessor
        // progress in quiet mode), so both pipes go through the same parser.
        let (line, from_stderr) = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                tree.kill();
                let _ = child.wait().await;
                return JobState::Cancelled;
            }
            line = next_line(&mut out, &mut obuf), if !out_done => match line {
                Some(l) => (l, false),
                None => { out_done = true; continue }
            },
            line = next_line(&mut err, &mut ebuf), if !err_done => match line {
                Some(l) => (l, true),
                None => { err_done = true; continue }
            },
        };
        match parse_line(&line) {
            Line::Download(d) => {
                p.set_state(JobState::Downloading);
                let finished = d.status == DownloadStatus::Finished;
                downloaded_any |= finished;
                p.progress(overall.fraction(&d), d.speed, d.eta, finished);
            }
            Line::Postprocess {
                status: PostprocessStatus::Started,
                name,
            } if downloaded_any => {
                p.set_state(JobState::Converting(postprocess_label(&name, &spec.format)));
                p.progress(None, None, None, true);
            }
            Line::Formats { format_ids, sizes } => overall.set_formats(format_ids, &sizes),
            Line::PlannedFile(planned) if spec.on_conflict == ConflictPolicy::Skip => {
                let dest = destination(spec, work, &planned, final_ext);
                if dest.exists() {
                    tree.kill();
                    let _ = child.wait().await;
                    return JobState::Skipped(dest);
                }
            }
            Line::FinalFile(f) => final_file = Some(f),
            Line::Other if from_stderr => {
                if stderr_tail.len() >= STDERR_KEEP {
                    stderr_tail.pop_front();
                }
                stderr_tail.push_back(line);
            }
            _ => {}
        }
    }

    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tree.kill();
            let _ = child.wait().await;
            return JobState::Cancelled;
        }
        s = child.wait() => s,
    };
    let lines: Vec<String> = stderr_tail.into_iter().collect();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => return JobState::Failed(classify(&lines)),
        Err(e) => return JobState::Failed(UserError::new(format!("yt-dlp failed: {e}"))),
    }

    let produced = match final_file.filter(|f| f.is_file()) {
        Some(f) => f,
        None => match find_output(work, final_ext) {
            Some(f) => f,
            None => {
                return JobState::Failed(
                    UserError::new("yt-dlp finished but produced no file.")
                        .with_hint(crate::ytdlp::errors::UPDATE_HINT)
                        .with_detail(lines.join("\n")),
                );
            }
        },
    };

    let produced = if spec.format.is_audio() {
        produced
    } else {
        match ensure_compatible_mp4(tools, &produced, cancel, &mut p).await {
            Ok(f) => f,
            Err(None) => return JobState::Cancelled,
            Err(Some(e)) => return JobState::Failed(e),
        }
    };

    p.set_state(JobState::Converting("Saving".into()));
    let dest = destination(spec, work, &produced, final_ext);
    let policy = spec.on_conflict;
    match tokio::task::spawn_blocking(move || move_into_place(&produced, &dest, policy)).await {
        Ok(Ok(state)) => state,
        Ok(Err(e)) => JobState::Failed(e),
        Err(e) => JobState::Failed(UserError::new(format!("Saving failed: {e}"))),
    }
}

/// Fallback if the final path line was missing: the largest file with the
/// expected extension.
fn find_output(work: &Path, ext: &str) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![work.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
                && best.as_ref().is_none_or(|(size, _)| meta.len() > *size)
            {
                best = Some((meta.len(), path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Probe the merged file and convert it to H.264/AAC MP4 if it isn't already.
async fn ensure_compatible_mp4(
    tools: &ToolPaths,
    file: &Path,
    cancel: &CancellationToken,
    p: &mut Progress<'_>,
) -> Result<PathBuf, Option<UserError>> {
    let (Some(ffprobe), Some(ffmpeg)) = (tools.ffprobe.as_deref(), tools.ffmpeg.as_deref()) else {
        return Err(Some(UserError::new("ffmpeg is missing.")));
    };
    p.set_state(JobState::Converting("Checking codecs".into()));
    let info = ffmpeg::probe(ffprobe, file).await.map_err(Some)?;
    let Some(plan) = ffmpeg::mp4_plan(&info) else {
        return Ok(file.to_path_buf());
    };
    let label = if ffmpeg::plan_reencodes_video(&plan) {
        "Converting to H.264 (slow)"
    } else {
        "Converting to MP4"
    };
    p.set_state(JobState::Converting(label.into()));
    p.progress(Some(0.0), None, None, true);

    let output = file.with_extension("converted.mp4");
    let report = p.report;
    let last = Mutex::new(Instant::now() - PROGRESS_INTERVAL);
    let on_progress = |f: f32| {
        let mut last = last.lock().unwrap();
        if last.elapsed() >= PROGRESS_INTERVAL {
            *last = Instant::now();
            report(JobEvent::Progress {
                fraction: Some(f),
                speed: None,
                eta: None,
            });
        }
    };
    ffmpeg::convert(
        ffmpeg,
        file,
        &output,
        &plan,
        info.duration,
        cancel,
        &on_progress,
    )
    .await?;
    let _ = tokio::fs::remove_file(file).await;
    let final_path = file.with_extension("mp4");
    tokio::fs::rename(&output, &final_path).await.map_err(|e| {
        Some(UserError::new(format!(
            "Couldn't rename converted file: {e}"
        )))
    })?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::command::{AudioFormat, Options};

    fn spec(dir: &Path) -> JobSpec {
        JobSpec {
            id: 1,
            url: "https://www.youtube.com/watch?v=dPZTh2NKTm4".into(),
            video_id: None,
            title: "t".into(),
            channel: None,
            duration: None,
            album: None,
            format: OutputFormat::Audio {
                format: AudioFormat::Mp3,
                kbps: 192,
            },
            options: Options::default(),
            output_dir: dir.to_path_buf(),
            template: "%(title)s.%(ext)s".into(),
            on_conflict: ConflictPolicy::Rename,
        }
    }

    #[test]
    fn destination_maps_work_dir_into_output() {
        let work = Path::new(r"C:\tmp\job-1");
        let s = spec(Path::new(r"D:\Music"));
        assert_eq!(
            destination(&s, work, Path::new(r"C:\tmp\job-1\Song ｜ 🎵.webm"), "mp3"),
            PathBuf::from(r"D:\Music\Song ｜ 🎵.mp3")
        );
        assert_eq!(
            destination(&s, work, Path::new(r"C:\tmp\job-1\Artist\Song.mp3"), "mp3"),
            PathBuf::from(r"D:\Music\Artist\Song.mp3")
        );
        // A path outside the work dir only keeps its name.
        assert_eq!(
            destination(&s, work, Path::new(r"C:\elsewhere\x.mp3"), "mp3"),
            PathBuf::from(r"D:\Music\x.mp3")
        );
    }

    #[test]
    fn move_respects_conflict_policy() {
        let dir = std::env::temp_dir().join(format!("ytlocal-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out").join("song.mp3");

        let src = dir.join("a.mp3");
        std::fs::write(&src, b"one").unwrap();
        assert_eq!(
            move_into_place(&src, &dest, ConflictPolicy::Rename).unwrap(),
            JobState::Done(dest.clone())
        );
        assert!(!src.exists());

        std::fs::write(&src, b"two").unwrap();
        let second = dir.join("out").join("song (1).mp3");
        assert_eq!(
            move_into_place(&src, &dest, ConflictPolicy::Rename).unwrap(),
            JobState::Done(second.clone())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");

        std::fs::write(&src, b"three").unwrap();
        assert_eq!(
            move_into_place(&src, &dest, ConflictPolicy::Skip).unwrap(),
            JobState::Skipped(dest.clone())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"one");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
