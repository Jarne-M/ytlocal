#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ytlocal::config::Config;

/// A red rounded square with a white down arrow, drawn in code.
fn icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded-rectangle coverage with radius 14.
            let r = 14.0;
            let dx = (r - fx).max(fx - (N as f32 - r)).max(0.0);
            let dy = (r - fy).max(fy - (N as f32 - r)).max(0.0);
            let inside = (dx * dx + dy * dy).sqrt() <= r;
            // Arrow: shaft and head, centered.
            let shaft = (28.0..36.0).contains(&fx) && (14.0..36.0).contains(&fy);
            let head = (32.0..=48.0).contains(&fy) && (fx - 32.0).abs() <= 48.0 - fy;
            let bar = (16.0..48.0).contains(&fx) && (50.0..54.0).contains(&fy);
            let px = &mut rgba[(y * N + x) * 4..][..4];
            if inside {
                px.copy_from_slice(if shaft || head || bar {
                    &[255, 255, 255, 255]
                } else {
                    &[220, 38, 38, 255]
                });
            }
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}

fn main() -> eframe::Result {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    let config = Config::load();
    // `ytlocal.exe <link>...` opens with those links ready to preview.
    let links: Vec<String> = std::env::args().skip(1).collect();
    let size = config.window_size.unwrap_or([1100.0, 760.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YtLocal")
            .with_inner_size(size)
            .with_min_inner_size([760.0, 520.0])
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "YtLocal",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(ytlocal::ui::App::new(
                cc,
                rt.handle().clone(),
                config,
                links,
            )))
        }),
    )
}
