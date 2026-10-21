//! Drives the real UI (egui_kittest) against the real backend: type links,
//! fetch previews, pick playlist items, download, check files and history.
//!
//! Needs the network and installed tools, so it's ignored by default:
//!   cargo test --test gui_e2e -- --ignored --nocapture

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ytlocal::config::Config;
use ytlocal::ui::App;

const VIDEO: &str = "https://youtu.be/XAUoA7q_xnM";
const IN_PLAYLIST: &str =
    "https://www.youtube.com/watch?v=XKgR03OK68w&list=PLwqi5Us_JxzhaWr8DzMOoPrhr5Rf-XXal&index=1";

/// Save a rendered frame when `YTLOCAL_SHOTS` names a folder (for eyeballing).
fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Some(dir) = std::env::var_os("YTLOCAL_SHOTS") else {
        return;
    };
    h.run_steps(2);
    match h.render() {
        Ok(img) => {
            let path = PathBuf::from(dir).join(format!("{name}.png"));
            img.save(&path).unwrap();
            println!("rendered {}", path.display());
        }
        Err(e) => println!("render failed: {e}"),
    }
}

fn wait_until(
    h: &mut Harness<'_, App>,
    secs: u64,
    what: &str,
    cond: impl Fn(&Harness<'_, App>) -> bool,
) {
    let start = Instant::now();
    loop {
        h.step();
        if cond(h) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore]
fn gui_end_to_end() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("ytlocal-gui-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("YTLOCAL_DATA_DIR", root.join("data")) };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = Config {
        audio_dir: root.join("audio"),
        video_dir: root.join("video"),
        ..Config::default()
    };

    let handle = rt.handle().clone();
    // The harness has no system fonts, so icon glyphs like ⚙ would panic.
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        .allow_missing_glyphs()
        .wgpu()
        .build_eframe(move |cc| App::new(cc, handle, config, Vec::new()));

    wait_until(&mut h, 120, "tools", |h| h.state().tools_ready());

    // Type two links and fetch their info.
    let input = h.get_by_role(Role::MultilineTextInput);
    input.focus();
    input.type_text(&format!("{VIDEO}\n{IN_PLAYLIST}"));
    h.run_steps(3);
    h.get_by_label("Fetch info").click();
    wait_until(&mut h, 90, "previews", |h| {
        h.query_by_label_contains("Download 2 items").is_some()
    });
    assert!(h.query_by_label_contains("short video of a bell").is_some());
    println!("previews shown: 2 videos");

    // Switch the second link to its playlist and pick two entries.
    h.get_by_label("Whole playlist").click();
    wait_until(&mut h, 90, "playlist", |h| {
        h.query_by_label_contains(" selected").is_some()
    });
    h.get_by_label("None").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("Download 1 item").is_some());
    h.get_by_label("1. Bobcat").click();
    h.run_steps(2);
    h.get_by_label("2. North American River Otter").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("Download 3 items").is_some());
    println!("playlist: picked 2 entries");
    shot(&mut h, "1_previews");

    h.get_by_label_contains("Download 3 items").click();
    wait_until(&mut h, 60, "download start", |h| {
        h.query_by_label_contains("Downloading").is_some()
            || h.query_by_label_contains("Converting").is_some()
    });
    shot(&mut h, "2_queue_active");
    wait_until(&mut h, 300, "downloads", |h| {
        let (active, done, bad) = h.state().job_counts();
        active == 0 && done + bad == 3
    });
    let (_, done, bad) = h.state().job_counts();
    assert_eq!((done, bad), (3, 0), "all three downloads should succeed");

    let files = h.state().finished_files();
    for f in &files {
        let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        println!("saved {} ({size} bytes)", f.display());
        assert!(f.starts_with(root.join("audio")) && size > 10_000);
    }
    assert_eq!(files.len(), 3);
    shot(&mut h, "3_queue_done");

    // History tab lists them, and was persisted to the isolated data dir.
    h.get_by_label("History").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Last 3 downloads").is_some());
    shot(&mut h, "4_history");

    h.get_by_label("⚙ Settings").click();
    h.run_steps(3);
    assert!(h.query_by_label("Parallel downloads").is_some());
    shot(&mut h, "5_settings");
    h.get_by_label("⚙ Settings").click();
    h.run_steps(2);

    // Video mode shows the quality picker and subtitle option.
    h.get_by_label("🎬 Video (MP4)").click();
    h.run_steps(2);
    assert!(h.query_by_label("Quality").is_some());
    assert!(h.query_by_label("Subtitles").is_some());
    shot(&mut h, "6_video_mode");
    wait_until(&mut h, 10, "history.json", |_| {
        root.join("data").join("history.json").exists()
    });

    // Nothing left in the work folder for these jobs.
    drop(h);
    let _ = std::fs::remove_dir_all(&root);
}
