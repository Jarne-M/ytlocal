//! Parsing yt-dlp's stdout. We ask yt-dlp to print machine-readable lines
//! with our own `YTL*` prefixes (see the templates below); every other line
//! is ignored, so new or changed yt-dlp messages can't break us.

use std::path::PathBuf;

use serde_json::Value;

/// `%(progress)j` is the progress-hook dict as JSON; `info.format_id` tells
/// which part (video or audio) of a merged download is in flight.
pub const DOWNLOAD_TEMPLATE: &str = "download:YTLPROG %(info.format_id)s %(progress)j";
pub const POSTPROCESS_TEMPLATE: &str =
    "postprocess:YTLPP %(progress.status)s %(progress.postprocessor)s";
/// Printed once formats are chosen: "137+140|[sizes]|[approx sizes]".
pub const PRINT_FORMATS: &str = "video:YTLFMT %(format_id)s|%(requested_formats.:.filesize)j|%(requested_formats.:.filesize_approx)j";
/// Printed before downloading: the planned file name (pre-conversion extension).
pub const PRINT_NAME: &str = "video:YTLNAME %(filename)s";
/// Printed once the file is final (after every postprocessor).
pub const PRINT_FILE: &str = "after_move:YTLFILE %(filepath)s";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadStatus {
    Downloading,
    Finished,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DownloadProgress {
    pub format_id: Option<String>,
    pub status: DownloadStatus,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: Option<f64>,
    /// Seconds remaining.
    pub eta: Option<u64>,
    pub fragment: Option<(u64, u64)>,
}

impl DownloadProgress {
    /// Fraction of this part, 0..=1, if it can be known.
    pub fn fraction(&self) -> Option<f32> {
        if self.status == DownloadStatus::Finished {
            return Some(1.0);
        }
        if let Some(total) = self.total.filter(|t| *t > 0) {
            return Some((self.downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32);
        }
        self.fragment
            .filter(|(_, n)| *n > 0)
            .map(|(i, n)| (i as f32 / n as f32).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostprocessStatus {
    Started,
    Processing,
    Finished,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Download(DownloadProgress),
    Postprocess {
        status: PostprocessStatus,
        name: String,
    },
    Formats {
        format_ids: Vec<String>,
        sizes: Vec<Option<u64>>,
    },
    PlannedFile(PathBuf),
    FinalFile(PathBuf),
    /// Anything else: ignored.
    Other,
}

fn as_u64(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(s) => s.trim().parse::<f64>().ok().map(|f| f.max(0.0) as u64),
        _ => None,
    }
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn not_na(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s != "NA").then(|| s.to_string())
}

fn parse_download(rest: &str) -> Option<DownloadProgress> {
    let (format_id, json) = rest.split_once(' ')?;
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    let status = match v.get("status")?.as_str()? {
        "downloading" => DownloadStatus::Downloading,
        "finished" => DownloadStatus::Finished,
        "error" => DownloadStatus::Error,
        _ => return None,
    };
    let total = as_u64(v.get("total_bytes")).or_else(|| as_u64(v.get("total_bytes_estimate")));
    let fragment = as_u64(v.get("fragment_index")).zip(as_u64(v.get("fragment_count")));
    Some(DownloadProgress {
        format_id: not_na(format_id),
        status,
        downloaded: as_u64(v.get("downloaded_bytes")).unwrap_or(0),
        total,
        speed: as_f64(v.get("speed")).filter(|s| s.is_finite() && *s >= 0.0),
        eta: as_u64(v.get("eta")),
        fragment,
    })
}

fn parse_sizes(json: &str) -> Vec<Option<u64>> {
    match serde_json::from_str::<Value>(json.trim()) {
        Ok(Value::Array(items)) => items.iter().map(|v| as_u64(Some(v))).collect(),
        _ => Vec::new(),
    }
}

pub fn parse_line(line: &str) -> Line {
    let line = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = line.strip_prefix("YTLPROG ") {
        return parse_download(rest).map_or(Line::Other, Line::Download);
    }
    if let Some(rest) = line.strip_prefix("YTLPP ") {
        let (status, name) = rest.split_once(' ').unwrap_or((rest, ""));
        let status = match status {
            "started" => PostprocessStatus::Started,
            "processing" => PostprocessStatus::Processing,
            "finished" => PostprocessStatus::Finished,
            _ => return Line::Other,
        };
        return Line::Postprocess {
            status,
            name: name.trim().to_string(),
        };
    }
    if let Some(rest) = line.strip_prefix("YTLFMT ") {
        let mut parts = rest.splitn(3, '|');
        let ids = parts.next().unwrap_or("");
        let format_ids = ids.split('+').filter_map(not_na).collect::<Vec<_>>();
        let exact = parse_sizes(parts.next().unwrap_or(""));
        let approx = parse_sizes(parts.next().unwrap_or(""));
        let sizes = (0..format_ids.len())
            .map(|i| {
                exact
                    .get(i)
                    .copied()
                    .flatten()
                    .or(approx.get(i).copied().flatten())
            })
            .collect();
        return Line::Formats { format_ids, sizes };
    }
    for (prefix, make) in [
        ("YTLFILE ", Line::FinalFile as fn(PathBuf) -> Line),
        ("YTLNAME ", Line::PlannedFile),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let path = rest.trim();
            if !path.is_empty() && path != "NA" {
                return make(PathBuf::from(path));
            }
        }
    }
    Line::Other
}

/// Combines per-part progress of a merged (video+audio) download into one
/// fraction, weighting parts by size when yt-dlp knows them.
#[derive(Clone, Debug, Default)]
pub struct OverallProgress {
    format_ids: Vec<String>,
    weights: Vec<f32>,
}

impl OverallProgress {
    pub fn set_formats(&mut self, format_ids: Vec<String>, sizes: &[Option<u64>]) {
        let n = format_ids.len().max(1);
        let known: Option<Vec<u64>> = sizes.iter().copied().collect();
        self.weights = match known {
            Some(s) if s.len() == format_ids.len() && s.iter().sum::<u64>() > 0 => {
                let sum = s.iter().sum::<u64>() as f32;
                s.iter().map(|x| *x as f32 / sum).collect()
            }
            _ => vec![1.0 / n as f32; n],
        };
        self.format_ids = format_ids;
    }

    pub fn fraction(&self, p: &DownloadProgress) -> Option<f32> {
        let part = p.fraction()?;
        if self.format_ids.len() <= 1 {
            return Some(part);
        }
        let Some(idx) = p
            .format_id
            .as_ref()
            .and_then(|id| self.format_ids.iter().position(|f| f == id))
        else {
            return Some(part);
        };
        let before: f32 = self.weights[..idx].iter().sum();
        Some((before + self.weights[idx] * part).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"YTLPROG 251 {"status": "downloading", "downloaded_bytes": 31744, "total_bytes": 85294, "tmpfilename": "C:\\tmp\\x.webm.part", "filename": "C:\\tmp\\x.webm", "eta": 0, "speed": 15842930.29224179, "elapsed": 0.179, "ctx_id": null, "_eta_str": "00:00", "_percent": 37.2}"#;

    #[test]
    fn parses_real_progress_line() {
        let Line::Download(p) = parse_line(REAL) else {
            panic!("not parsed")
        };
        assert_eq!(p.format_id.as_deref(), Some("251"));
        assert_eq!(p.status, DownloadStatus::Downloading);
        assert_eq!(p.downloaded, 31744);
        assert_eq!(p.total, Some(85294));
        assert_eq!(p.eta, Some(0));
        assert!((p.fraction().unwrap() - 0.3721).abs() < 0.001);
    }

    #[test]
    fn estimate_nulls_and_fragments() {
        let l = r#"YTLPROG NA {"status": "downloading", "downloaded_bytes": 500, "total_bytes": null, "total_bytes_estimate": 1000.0, "speed": null, "eta": null}"#;
        let Line::Download(p) = parse_line(l) else {
            panic!()
        };
        assert_eq!(p.format_id, None);
        assert_eq!(p.total, Some(1000));
        assert_eq!(p.speed, None);
        assert_eq!(p.fraction(), Some(0.5));

        let l = r#"YTLPROG 18 {"status": "downloading", "downloaded_bytes": 10, "fragment_index": 3, "fragment_count": 12}"#;
        let Line::Download(p) = parse_line(l) else {
            panic!()
        };
        assert_eq!(p.fraction(), Some(0.25));

        let l = r#"YTLPROG 18 {"status": "finished", "downloaded_bytes": 10}"#;
        let Line::Download(p) = parse_line(l) else {
            panic!()
        };
        assert_eq!(p.fraction(), Some(1.0));
    }

    #[test]
    fn unknown_and_broken_lines_are_ignored() {
        assert_eq!(
            parse_line("[youtube] Extracting URL: https://..."),
            Line::Other
        );
        assert_eq!(parse_line("YTLPROG 251 {not json"), Line::Other);
        assert_eq!(
            parse_line(r#"YTLPROG 251 {"status": "weird"}"#),
            Line::Other
        );
        assert_eq!(parse_line("YTLPP exploded Foo"), Line::Other);
        assert_eq!(parse_line(""), Line::Other);
        assert_eq!(parse_line("YTLFILE NA"), Line::Other);
    }

    #[test]
    fn postprocess_formats_and_file() {
        assert_eq!(
            parse_line("YTLPP started ExtractAudio\r"),
            Line::Postprocess {
                status: PostprocessStatus::Started,
                name: "ExtractAudio".into()
            }
        );
        assert_eq!(
            parse_line("YTLFMT 137+140|[1000, null]|[1100, 300]"),
            Line::Formats {
                format_ids: vec!["137".into(), "140".into()],
                sizes: vec![Some(1000), Some(300)]
            }
        );
        assert_eq!(
            parse_line("YTLFMT 251|[]|[]"),
            Line::Formats {
                format_ids: vec!["251".into()],
                sizes: vec![None]
            }
        );
        assert_eq!(
            parse_line("YTLFILE C:\\Users\\me\\tmp\\Ünï 🎵 𝕐.mp3"),
            Line::FinalFile(PathBuf::from("C:\\Users\\me\\tmp\\Ünï 🎵 𝕐.mp3"))
        );
        assert_eq!(
            parse_line("YTLNAME C:\\tmp\\a.webm"),
            Line::PlannedFile(PathBuf::from("C:\\tmp\\a.webm"))
        );
    }

    #[test]
    fn overall_progress_weights_parts() {
        let mut o = OverallProgress::default();
        o.set_formats(vec!["137".into(), "140".into()], &[Some(900), Some(100)]);
        let p = |id: &str, done: u64, total: u64| DownloadProgress {
            format_id: Some(id.into()),
            status: DownloadStatus::Downloading,
            downloaded: done,
            total: Some(total),
            speed: None,
            eta: None,
            fragment: None,
        };
        assert!((o.fraction(&p("137", 450, 900)).unwrap() - 0.45).abs() < 1e-6);
        assert!((o.fraction(&p("140", 50, 100)).unwrap() - 0.95).abs() < 1e-6);

        // Unknown sizes: equal weights.
        o.set_formats(vec!["a".into(), "b".into()], &[None, Some(5)]);
        assert!((o.fraction(&p("b", 0, 10)).unwrap() - 0.5).abs() < 1e-6);
        // Unknown part id: fall back to the part's own fraction.
        assert!((o.fraction(&p("zzz", 3, 10)).unwrap() - 0.3).abs() < 1e-6);
    }
}
