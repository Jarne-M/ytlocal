//! Parsing `yt-dlp -J` output into preview data.

use serde_json::Value;

use super::url::Link;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistEntry {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    /// False for private/deleted entries; they can't be downloaded.
    pub available: bool,
}

impl PlaylistEntry {
    pub fn url(&self) -> String {
        Link::video_url(&self.id)
    }
    pub fn thumbnail(&self) -> String {
        Link::thumbnail_url(&self.id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Video(VideoInfo),
    Playlist(PlaylistInfo),
}

fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| v.get(*k)?.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn duration(v: &Value) -> Option<f64> {
    v.get("duration")?.as_f64().filter(|d| *d >= 0.0)
}

const UNAVAILABLE_TITLES: [&str; 3] = ["[Private video]", "[Deleted video]", "[Unavailable video]"];

/// Parse a `-J` document. Errors are user-facing messages.
pub fn parse(json: &str) -> Result<Preview, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|_| "yt-dlp returned unreadable metadata".to_string())?;
    let kind = v.get("_type").and_then(Value::as_str).unwrap_or("video");
    if kind == "playlist" || v.get("entries").is_some_and(Value::is_array) {
        let entries = v
            .get("entries")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|e| !e.is_null())
                    .filter_map(|e| {
                        let id = str_field(e, &["id"])?;
                        let title = str_field(e, &["title"]).unwrap_or_else(|| id.clone());
                        let restricted = matches!(
                            e.get("availability").and_then(Value::as_str),
                            Some("private" | "needs_auth" | "subscriber_only" | "premium_only")
                        );
                        let available =
                            !restricted && !UNAVAILABLE_TITLES.contains(&title.as_str());
                        Some(PlaylistEntry {
                            channel: str_field(e, &["channel", "uploader"]),
                            duration: duration(e),
                            available,
                            id,
                            title,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        return Ok(Preview::Playlist(PlaylistInfo {
            id: str_field(&v, &["id"]).unwrap_or_default(),
            title: str_field(&v, &["title"]).unwrap_or_else(|| "Playlist".into()),
            channel: str_field(&v, &["channel", "uploader"]),
            entries,
        }));
    }

    match v.get("live_status").and_then(Value::as_str) {
        Some("is_live") => {
            return Err(
                "This is a live stream that's still running; download it after it ends.".into(),
            );
        }
        Some("is_upcoming") => {
            return Err(
                "This is an upcoming premiere or live stream; it can't be downloaded yet.".into(),
            );
        }
        _ => {}
    }
    let id = str_field(&v, &["id"]).ok_or("yt-dlp returned no video id")?;
    Ok(Preview::Video(VideoInfo {
        title: str_field(&v, &["title", "fulltitle"]).unwrap_or_else(|| id.clone()),
        channel: str_field(&v, &["channel", "uploader", "artist"]),
        duration: duration(&v),
        thumbnail: Link::thumbnail_url(&id),
        url: Link::video_url(&id),
        id,
    }))
}

/// "1:02:03" / "4:05" for display.
pub fn format_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video() {
        let json = r#"{"id": "dPZTh2NKTm4", "title": "What is Creative Commons? 🎵", "channel": "Wikipedia",
            "duration": 84, "live_status": "not_live", "_type": "video"}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert_eq!(v.title, "What is Creative Commons? 🎵");
        assert_eq!(v.channel.as_deref(), Some("Wikipedia"));
        assert_eq!(v.duration, Some(84.0));
        assert_eq!(v.url, "https://www.youtube.com/watch?v=dPZTh2NKTm4");
        assert!(v.thumbnail.contains("dPZTh2NKTm4"));
    }

    #[test]
    fn video_falls_back_to_uploader() {
        let json = r#"{"id": "abcdefghijk", "title": "", "uploader": "Someone", "duration": null}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert_eq!(v.title, "abcdefghijk");
        assert_eq!(v.channel.as_deref(), Some("Someone"));
        assert_eq!(v.duration, None);
    }

    #[test]
    fn live_is_rejected() {
        assert!(
            parse(r#"{"id": "abcdefghijk", "live_status": "is_live"}"#)
                .unwrap_err()
                .contains("live")
        );
        assert!(parse(r#"{"id": "abcdefghijk", "live_status": "is_upcoming"}"#).is_err());
    }

    #[test]
    fn flat_playlist() {
        let json = r#"{"_type": "playlist", "id": "PL1", "title": "My Mix", "uploader": "Me", "entries": [
            {"_type": "url", "id": "aaaaaaaaaaa", "title": "One", "duration": 61.0, "channel": "A"},
            {"_type": "url", "id": "bbbbbbbbbbb", "title": "[Private video]", "duration": null},
            {"_type": "url", "id": "ccccccccccc", "title": "Three", "availability": "needs_auth"},
            null,
            {"_type": "url", "title": "no id"}
        ]}"#;
        let Ok(Preview::Playlist(p)) = parse(json) else {
            panic!()
        };
        assert_eq!(p.title, "My Mix");
        assert_eq!(p.channel.as_deref(), Some("Me"));
        assert_eq!(p.entries.len(), 3);
        assert!(p.entries[0].available);
        assert!(!p.entries[1].available);
        assert!(!p.entries[2].available);
        assert_eq!(
            p.entries[0].url(),
            "https://www.youtube.com/watch?v=aaaaaaaaaaa"
        );
    }

    #[test]
    fn garbage() {
        assert!(parse("not json").is_err());
        assert!(parse("{}").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(84.0), "1:24");
        assert_eq!(format_duration(3723.4), "1:02:03");
        assert_eq!(format_duration(5.0), "0:05");
    }
}
