//! Settings changed in the UI are saved to config.json and restored on the
//! next start. Offline; uses an isolated settings folder.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ytlocal::config::{Config, Mode};
use ytlocal::queue::job::ConflictPolicy;
use ytlocal::ui::App;

fn app(rt: &tokio::runtime::Runtime, config: Config) -> Harness<'static, App> {
    let handle = rt.handle().clone();
    Harness::builder()
        .with_size(egui::vec2(1200.0, 900.0))
        .allow_missing_glyphs()
        .build_eframe(move |cc| App::new(cc, handle, config, Vec::new()))
}

#[test]
fn settings_persist_across_restarts() {
    let root = std::env::temp_dir().join(format!("ytlocal-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("YTLOCAL_DATA_DIR", &root) };
    let config_path = root.join("config.json");
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut h = app(&rt, Config::load());
    h.run_steps(2);
    h.get_by_label("🎬 Video (MP4)").click();
    h.run_steps(2);
    h.get_by_label("⚙ Settings").click();
    h.run_steps(2);
    h.get_by_label("Skip").click();
    h.run_steps(2);

    // Autosave is debounced; wait for the file.
    let start = Instant::now();
    let saved = loop {
        h.step();
        let c = Config::load_from(&config_path);
        if config_path.exists() && c.mode == Mode::Video && c.on_conflict == ConflictPolicy::Skip {
            break c;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "config.json was not saved"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(h);

    // A fresh start restores the choices.
    let mut h = app(&rt, Config::load());
    h.run_steps(2);
    assert_eq!(h.state().job_counts(), (0, 0, 0));
    assert!(h.query_by_label("Quality").is_some(), "video mode restored");
    assert_eq!(Config::load(), saved);

    drop(h);
    drop(rt);
    let _ = std::fs::remove_dir_all(&root);
}
