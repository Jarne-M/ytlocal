//! Final file names: Windows-safe, length-limited, never overwriting.
//!
//! yt-dlp already sanitizes the names it produces; this is the last line of
//! defence before a file lands in the user's folder.

use std::path::{Component, Path, PathBuf};

/// Windows path components are limited to 255 UTF-16 units.
const MAX_COMPONENT: usize = 255;
/// Keep whole paths under the classic MAX_PATH so Explorer and older apps cope.
const MAX_PATH: usize = 259;
/// Never truncate a stem below this many UTF-16 units to make a path fit.
const MIN_STEM: usize = 24;

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Truncate to at most `max` UTF-16 units, on a char boundary.
fn truncate_utf16(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &s[..i];
        }
    }
    s
}

/// Split "name.ext" into ("name", ".ext"). Hidden-style names have no extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 6 && !name[i + 1..].contains(' ') => {
            (&name[..i], &name[i..])
        }
        _ => (name, ""),
    }
}

/// Make one path component valid on Windows.
pub fn sanitize_component(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    // Trailing dots and spaces are silently dropped by Windows; leading spaces are confusing.
    out = out.trim_start().trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        out = "download".to_string();
    }
    let (stem, _) = split_ext(&out);
    if RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem.trim_end()))
    {
        out.insert(0, '_');
    }
    shorten(&out, MAX_COMPONENT)
}

/// Shorten a file name to `max` UTF-16 units, keeping its extension.
pub fn shorten(name: &str, max: usize) -> String {
    if utf16_len(name) <= max {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let room = max.saturating_sub(utf16_len(ext)).max(1);
    let stem = truncate_utf16(stem, room).trim_end_matches(['.', ' ']);
    format!("{stem}{ext}")
}

/// Sanitize every component of a relative path (templates may contain
/// sub-folders), dropping anything that could escape the output folder.
pub fn sanitize_relative(rel: &Path) -> PathBuf {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(sanitize_component(&s.to_string_lossy())),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return PathBuf::from("download");
    }
    parts.iter().collect()
}

/// Destination for `rel` inside `dir`, with the file name shortened if the
/// whole path would exceed MAX_PATH.
pub fn fit_in(dir: &Path, rel: &Path) -> PathBuf {
    let full = dir.join(rel);
    let len = utf16_len(&full.to_string_lossy());
    if len <= MAX_PATH {
        return full;
    }
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let over = len - MAX_PATH;
    let (stem, ext) = split_ext(&name);
    let keep = utf16_len(stem).saturating_sub(over).max(MIN_STEM);
    let short = shorten(&name, keep + utf16_len(ext));
    full.with_file_name(short)
}

/// `path` if free, otherwise "name (1).ext", "name (2).ext", ...
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = split_ext(&name);
    (1..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("infinite iterator")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_invalid_characters() {
        assert_eq!(
            sanitize_component("a<b>c:d\"e/f\\g|h?i*j.mp3"),
            "a_b_c_d_e_f_g_h_i_j.mp3"
        );
        assert_eq!(sanitize_component("tab\there\u{1}.mp3"), "tab_here_.mp3");
    }

    #[test]
    fn trims_dots_and_spaces() {
        assert_eq!(sanitize_component("  name. . "), "name");
        assert_eq!(sanitize_component("..."), "download");
        assert_eq!(sanitize_component(""), "download");
    }

    #[test]
    fn reserved_names() {
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("con.mp3"), "_con.mp3");
        assert_eq!(sanitize_component("LPT1.mp4"), "_LPT1.mp4");
        assert_eq!(sanitize_component("CONSOLE.mp3"), "CONSOLE.mp3");
    }

    #[test]
    fn keeps_unicode_and_emoji() {
        let name = "Ünïcödé 日本語 🎵🔥 𝕐.mp3";
        assert_eq!(sanitize_component(name), name);
    }

    #[test]
    fn shortens_long_names_keeping_extension() {
        let long = format!("{}.mp3", "x".repeat(400));
        let s = sanitize_component(&long);
        assert_eq!(utf16_len(&s), 255);
        assert!(s.ends_with(".mp3"));

        // Emoji are 2 UTF-16 units; never split one.
        let emoji = format!("{}.opus", "🎵".repeat(200));
        let s = shorten(&emoji, 100);
        assert!(utf16_len(&s) <= 100);
        assert!(s.ends_with(".opus"));
        assert!(s.trim_end_matches(".opus").chars().all(|c| c == '🎵'));
    }

    #[test]
    fn extension_detection() {
        assert_eq!(split_ext("song.mp3"), ("song", ".mp3"));
        assert_eq!(
            split_ext("Mr. Smith goes to town"),
            ("Mr. Smith goes to town", "")
        );
        assert_eq!(split_ext(".hidden"), (".hidden", ""));
    }

    #[test]
    fn relative_paths_cannot_escape() {
        let p = sanitize_relative(Path::new("../..\\evil/na:me.mp3"));
        assert_eq!(p, PathBuf::from("evil").join("na_me.mp3"));
        assert_eq!(
            sanitize_relative(Path::new("..")),
            PathBuf::from("download")
        );
    }

    #[test]
    fn fits_long_paths() {
        let dir = PathBuf::from(format!("C:\\{}", "d".repeat(150)));
        let rel = PathBuf::from(format!("{}.mp4", "t".repeat(200)));
        let p = fit_in(&dir, &rel);
        assert!(utf16_len(&p.to_string_lossy()) <= MAX_PATH);
        assert!(p.to_string_lossy().ends_with(".mp4"));
        let short = PathBuf::from("ok.mp4");
        assert_eq!(fit_in(&dir, &short), dir.join("ok.mp4"));
    }

    #[test]
    fn unique_paths_add_suffix() {
        let dir = std::env::temp_dir().join(format!("ytlocal-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("song.mp3");
        assert_eq!(unique_path(&p), p);
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(unique_path(&p), dir.join("song (1).mp3"));
        std::fs::write(dir.join("song (1).mp3"), b"x").unwrap();
        assert_eq!(unique_path(&p), dir.join("song (2).mp3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
