//! Drive the backend without the GUI, for end-to-end testing.
//!
//!   cargo run --example headless -- tools
//!   cargo run --example headless -- install ytdlp|ffmpeg|deno
//!   cargo run --example headless -- update
//!   cargo run --example headless -- preview <url> [playlist]
//!   cargo run --example headless -- download <out-dir> <format> <url>... [--normalize] [--trim a-b]
//!       [--album NAME] [--subs] [--skip] [--no-meta] [--h264-any] [--cancel-after SECS]
//!       format: mp3-128..mp3-320 | m4a | opus | wav | flac | mp4-best | mp4-1080 | mp4-720 | mp4-480 | mp4-360

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ytlocal::backend::{Backend, Event, Request};
use ytlocal::binaries::Package;
use ytlocal::queue::job::{ConflictPolicy, JobEvent, JobSpec, JobState};
use ytlocal::ytdlp::command::{AudioFormat, Options, OutputFormat, VideoQuality, parse_trim};

fn wait_tools(backend: &Backend) -> bool {
    let mut ok = true;
    let mut last_stage = String::new();
    while let Some(ev) = backend.recv() {
        match ev {
            Event::ToolProgress { pkg, progress } if progress.stage != last_stage => {
                println!("[{}] {}", pkg.label(), progress.stage);
                last_stage = progress.stage.clone();
            }
            Event::ToolFinished { pkg, result } => {
                println!("[{}] finished: {result:?}", pkg.label());
                ok = result.is_ok();
            }
            Event::Tools(report) => {
                for tool in ytlocal::binaries::Tool::ALL {
                    println!("{:8} {:?}", tool.label(), report.get(tool));
                }
                return ok;
            }
            _ => {}
        }
    }
    ok
}

fn parse_format(s: &str) -> OutputFormat {
    let audio = |format, kbps| OutputFormat::Audio { format, kbps };
    let video = |quality| OutputFormat::Video { quality };
    match s {
        "m4a" => audio(AudioFormat::M4a, 0),
        "opus" => audio(AudioFormat::Opus, 0),
        "wav" => audio(AudioFormat::Wav, 0),
        "flac" => audio(AudioFormat::Flac, 0),
        "mp4-best" => video(VideoQuality::Best),
        "mp4-1080" => video(VideoQuality::P1080),
        "mp4-720" => video(VideoQuality::P720),
        "mp4-480" => video(VideoQuality::P480),
        "mp4-360" => video(VideoQuality::P360),
        s if s.starts_with("mp3-") => audio(AudioFormat::Mp3, s[4..].parse().expect("kbps")),
        _ => panic!("unknown format {s}"),
    }
}

fn download(backend: &Backend, args: &[String]) -> bool {
    let out_dir = PathBuf::from(&args[0]);
    let format = parse_format(&args[1]);
    let mut options = Options::default();
    let mut urls = Vec::new();
    let mut album = None;
    let mut policy = ConflictPolicy::Rename;
    let mut cancel_after = None;
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--normalize" => options.normalize = true,
            "--subs" => options.subtitles = true,
            "--no-meta" => options.embed_metadata = false,
            "--h264-any" => options.prefer_h264 = false,
            "--skip" => policy = ConflictPolicy::Skip,
            "--rate" => options.rate_limit = Some(it.next().unwrap().parse().unwrap()),
            "--album" => album = it.next().cloned(),
            "--trim" => {
                let (s, e) = it.next().unwrap().split_once('-').unwrap();
                options.trim = parse_trim(s, e).unwrap();
            }
            "--cancel-after" => cancel_after = Some(it.next().unwrap().parse::<f64>().unwrap()),
            url => urls.push(url.to_string()),
        }
    }
    wait_tools(backend);

    let specs: Vec<JobSpec> = urls
        .iter()
        .enumerate()
        .map(|(i, url)| JobSpec {
            id: i as u64 + 1,
            url: url.clone(),
            video_id: None,
            title: url.clone(),
            channel: None,
            duration: None,
            album: album.clone(),
            format,
            options: options.clone(),
            output_dir: out_dir.clone(),
            template: "%(title)s.%(ext)s".into(),
            on_conflict: policy,
        })
        .collect();
    let n = specs.len();
    backend.send(Request::Enqueue(specs));

    let start = Instant::now();
    let mut finished = 0;
    let mut all_ok = true;
    let mut last_print = Instant::now();
    loop {
        if let Some(secs) = cancel_after
            && start.elapsed().as_secs_f64() > secs
        {
            println!("--- cancelling all after {secs}s");
            backend.send(Request::CancelAll);
            cancel_after = None;
        }
        let Some(ev) = (match backend.try_recv() {
            Some(ev) => Some(ev),
            None => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        }) else {
            break;
        };
        match ev {
            Event::Job {
                id,
                event: JobEvent::State(state),
            } => {
                println!(
                    "[{:6.2}s] job {id}: {state:?}",
                    start.elapsed().as_secs_f64()
                );
                if state.is_finished() {
                    finished += 1;
                    all_ok &= matches!(state, JobState::Done(_) | JobState::Skipped(_));
                }
            }
            Event::Job {
                id,
                event:
                    JobEvent::Progress {
                        fraction,
                        speed,
                        eta,
                    },
            } => {
                if last_print.elapsed() > Duration::from_millis(500) {
                    last_print = Instant::now();
                    println!(
                        "[{:6.2}s] job {id}: {:?} speed={:?} eta={eta:?}",
                        start.elapsed().as_secs_f64(),
                        fraction.map(|f| format!("{:.1}%", f * 100.0)),
                        speed.map(|s| format!("{:.0} KiB/s", s / 1024.0)),
                    );
                }
            }
            Event::QueueIdle { done, failed } => {
                println!("queue idle: done={done} failed={failed}");
                if finished >= n {
                    break;
                }
            }
            _ => {}
        }
    }
    all_ok
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let backend = Backend::start(rt.handle(), None, 2);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = match args.first().map(String::as_str) {
        Some("tools") | None => wait_tools(&backend),
        Some("install") => {
            wait_tools(&backend);
            let pkg = match args.get(1).map(String::as_str) {
                Some("ytdlp") => Package::YtDlp,
                Some("ffmpeg") => Package::Ffmpeg,
                Some("deno") => Package::Deno,
                _ => panic!("install ytdlp|ffmpeg|deno"),
            };
            backend.send(Request::Install(pkg));
            wait_tools(&backend)
        }
        Some("update") => {
            wait_tools(&backend);
            backend.send(Request::UpdateYtDlp);
            wait_tools(&backend)
        }
        Some("preview") => {
            wait_tools(&backend);
            let playlist = args.get(2).is_some_and(|a| a == "playlist");
            backend.send(Request::Preview {
                req: 1,
                url: args[1].clone(),
                playlist,
            });
            loop {
                if let Some(Event::Preview { result, .. }) = backend.recv() {
                    println!("{result:#?}");
                    break result.is_ok();
                }
            }
        }
        Some("download") => download(&backend, &args[1..]),
        Some("toast") => {
            let r = ytlocal::notify::toast("YtLocal test", "Downloads finished: 3 files saved");
            println!("toast: {r:?}");
            r.is_ok()
        }
        Some(other) => panic!("unknown command {other}"),
    };
    std::process::exit(if ok { 0 } else { 1 });
}
