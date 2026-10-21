//! Format, quality, extras, output folder and the Download button.

use super::App;
use crate::config::Mode;
use crate::ytdlp::command::{AudioFormat, MP3_BITRATES, VideoQuality};

pub fn options_section(app: &mut App, ui: &mut egui::Ui) {
    let c = &mut app.config;
    ui.separator();
    ui.strong("Format");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut c.mode, Mode::Audio, "🎵 Audio");
        ui.selectable_value(&mut c.mode, Mode::Video, "🎬 Video (MP4)");
    });
    ui.horizontal(|ui| match c.mode {
        Mode::Audio => {
            let label = ui.label("Codec");
            egui::ComboBox::from_id_salt("audio_format")
                .selected_text(c.audio_format.label())
                .show_ui(ui, |ui| {
                    for f in AudioFormat::ALL {
                        ui.selectable_value(&mut c.audio_format, f, f.label());
                    }
                })
                .response
                .labelled_by(label.id);
            if c.audio_format.has_bitrate() {
                let label = ui.label("Bitrate");
                egui::ComboBox::from_id_salt("kbps")
                    .selected_text(format!("{} kbps", c.mp3_kbps))
                    .show_ui(ui, |ui| {
                        for k in MP3_BITRATES {
                            ui.selectable_value(&mut c.mp3_kbps, k, format!("{k} kbps"));
                        }
                    })
                    .response
                    .labelled_by(label.id);
            } else {
                let note = match c.audio_format {
                    AudioFormat::Wav | AudioFormat::Flac => "lossless container",
                    _ => "best available, no re-encode",
                };
                ui.label(egui::RichText::new(note).weak());
            }
        }
        Mode::Video => {
            let label = ui.label("Quality");
            egui::ComboBox::from_id_salt("video_quality")
                .selected_text(c.video_quality.label())
                .show_ui(ui, |ui| {
                    for q in VideoQuality::ALL {
                        ui.selectable_value(&mut c.video_quality, q, q.label());
                    }
                })
                .response
                .labelled_by(label.id);
            ui.label(egui::RichText::new("H.264 + AAC, lower if unavailable").weak());
        }
    });

    ui.add_space(4.0);
    ui.strong("Options");
    let o = &mut c.options;
    match c.mode {
        Mode::Audio => {
            ui.checkbox(&mut o.embed_metadata, "Embed tags and cover art")
                .on_hover_text(
                    "Title, artist (channel), album (playlist name) and the thumbnail as cover",
                );
            ui.checkbox(&mut o.normalize, "Normalize volume")
                .on_hover_text("EBU R128 loudness normalization (-16 LUFS); forces a re-encode");
        }
        Mode::Video => {
            ui.checkbox(&mut o.embed_metadata, "Embed tags and thumbnail");
            ui.horizontal(|ui| {
                ui.checkbox(&mut o.subtitles, "Subtitles");
                ui.add_enabled(
                    o.subtitles,
                    egui::TextEdit::singleline(&mut o.subtitle_langs)
                        .desired_width(90.0)
                        .hint_text("en.*"),
                )
                .on_hover_text("Languages, comma-separated, e.g. en.*,nl");
                ui.add_enabled(
                    o.subtitles,
                    egui::Checkbox::new(&mut o.auto_subtitles, "auto-generated"),
                );
            });
        }
    }
    ui.horizontal(|ui| {
        ui.label("Trim");
        ui.add(
            egui::TextEdit::singleline(&mut app.trim_start)
                .desired_width(56.0)
                .hint_text("start"),
        );
        ui.label("–");
        ui.add(
            egui::TextEdit::singleline(&mut app.trim_end)
                .desired_width(56.0)
                .hint_text("end"),
        );
        ui.label(egui::RichText::new("m:ss, optional").weak());
    });

    ui.add_space(4.0);
    ui.strong(match app.config.mode {
        Mode::Audio => "Save audio to",
        Mode::Video => "Save video to",
    });
    ui.horizontal(|ui| {
        let dir = app.config.output_dir().to_path_buf();
        ui.add(egui::Label::new(dir.display().to_string()).truncate())
            .on_hover_text(dir.display().to_string());
    });
    ui.horizontal(|ui| {
        let busy = app.folder_pick.is_some();
        if ui
            .add_enabled(!busy, egui::Button::new("Change…"))
            .clicked()
        {
            let mode = app.config.mode;
            app.pick_folder(mode, ui.ctx());
        }
        if ui.button("Open folder").clicked() {
            let dir = app.config.output_dir().to_path_buf();
            let _ = std::fs::create_dir_all(&dir);
            super::queue::open_folder(&dir);
        }
    });

    ui.add_space(8.0);
    let n = app.input.total_selected();
    let label = match n {
        0 => "Download".to_string(),
        1 => "⬇ Download 1 item".to_string(),
        n => format!("⬇ Download {n} items"),
    };
    let button = egui::Button::new(egui::RichText::new(label).size(16.0))
        .min_size(egui::vec2(ui.available_width(), 34.0));
    let enabled = n > 0 && app.tools_ready();
    let resp = ui.add_enabled(enabled, button);
    if n == 0 && !app.input.is_loading() {
        resp.on_disabled_hover_text("Paste links and fetch their info first");
    } else if resp.clicked() {
        super::input::enqueue_ready(app);
    }
}
