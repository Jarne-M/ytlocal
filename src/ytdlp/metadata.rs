//! Parsing `yt-dlp -J` output into preview data.

use serde_json::Value;

use super::url::Link;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub url: String,
    /// Instagram/TikTok: name the file after this preview title, because
    /// yt-dlp's own title is useless ("Video by …") or cut off with "...".
    pub fixed_title: bool,
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

/// First caption line, shortened: Instagram's yt-dlp title is just "Video by
/// someone", TikTok's is the whole caption cut off at a fixed length.
fn caption_title(v: &Value) -> Option<String> {
    let caption = str_field(v, &["description"])?;
    let line = caption.lines().map(str::trim).find(|l| !l.is_empty())?;
    const MAX: usize = 80;
    Some(if line.chars().count() > MAX {
        format!("{}…", line.chars().take(MAX).collect::<String>().trim_end())
    } else {
        line.to_string()
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Social {
    Instagram,
    TikTok,
}

/// Instagram and TikTok: always a single video, with its own thumbnail and link.
fn parse_social(v: &Value, site: Social) -> Result<Preview, String> {
    if v.get("_type").and_then(Value::as_str) == Some("playlist")
        || v.get("entries").is_some_and(Value::is_array)
    {
        return Err(match site {
            Social::Instagram => "This Instagram post holds several photos/videos; YtLocal downloads single reels. Use a reel link.",
            Social::TikTok => "This TikTok link holds several videos or photos; YtLocal downloads single videos. Use a link to one video.",
        }
        .into());
    }
    let id = str_field(v, &["id"]).ok_or("yt-dlp returned no video id")?;
    let (channel, title) = match site {
        Social::Instagram => {
            let channel = str_field(v, &["channel", "uploader_id", "uploader"]);
            let title = str_field(v, &["title"])
                .filter(|t| !t.starts_with("Video by "))
                .or_else(|| caption_title(v))
                .or_else(|| channel.as_ref().map(|c| format!("Reel by {c}")));
            (channel, title)
        }
        Social::TikTok => {
            let channel = str_field(v, &["channel", "uploader", "creator"]);
            let title = caption_title(v)
                .or_else(|| str_field(v, &["title"]))
                .or_else(|| channel.as_ref().map(|c| format!("TikTok by {c}")));
            (channel, title)
        }
    };
    let url = match site {
        Social::Instagram => {
            str_field(v, &["webpage_url"]).unwrap_or_else(|| Link::instagram_url(&id))
        }
        Social::TikTok => str_field(v, &["webpage_url"])
            .unwrap_or_else(|| format!("https://www.tiktok.com/@/video/{id}")),
    };
    Ok(Preview::Video(VideoInfo {
        title: title.unwrap_or_else(|| id.clone()),
        channel,
        duration: duration(v),
        thumbnail: str_field(v, &["thumbnail"]),
        url,
        fixed_title: true,
        id,
    }))
}

/// Parse a `-J` document. Errors are user-facing messages.
pub fn parse(json: &str) -> Result<Preview, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|_| "yt-dlp returned unreadable metadata".to_string())?;
    match v.get("extractor_key").and_then(Value::as_str) {
        Some(k) if k.starts_with("Instagram") => return parse_social(&v, Social::Instagram),
        Some(k) if k.starts_with("TikTok") => return parse_social(&v, Social::TikTok),
        _ => {}
    }
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
        thumbnail: Some(Link::thumbnail_url(&id)),
        url: Link::video_url(&id),
        fixed_title: false,
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
        assert!(v.thumbnail.unwrap().contains("dPZTh2NKTm4"));
        assert!(!v.fixed_title);
    }

    #[test]
    fn instagram_reel() {
        let json = r#"{"id": "C8xYz12AbCd", "title": "Video by someone", "channel": "someone",
            "uploader": "Some One", "description": "\n  Sunset at the beach 🌅 #summer\nsecond line",
            "duration": 14.9, "thumbnail": "https://scontent.cdninstagram.com/v/t51/thumb.jpg?x=1",
            "webpage_url": "https://www.instagram.com/p/C8xYz12AbCd/", "extractor_key": "Instagram"}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert!(v.fixed_title);
        assert_eq!(v.title, "Sunset at the beach 🌅 #summer");
        assert_eq!(v.channel.as_deref(), Some("someone"));
        assert_eq!(v.url, "https://www.instagram.com/p/C8xYz12AbCd/");
        assert_eq!(
            v.thumbnail.as_deref(),
            Some("https://scontent.cdninstagram.com/v/t51/thumb.jpg?x=1")
        );

        // No caption: fall back to the uploader; no webpage_url: canonical link.
        let json = r#"{"id": "C8xYz12AbCd", "title": "Video by someone", "channel": "someone",
            "extractor_key": "Instagram"}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert_eq!(v.title, "Reel by someone");
        assert_eq!(v.url, "https://www.instagram.com/reel/C8xYz12AbCd/");
        assert_eq!(v.thumbnail, None);

        let long = format!(
            r#"{{"id": "C8xYz12AbCd", "description": "{}", "extractor_key": "Instagram"}}"#,
            "a".repeat(200)
        );
        let Ok(Preview::Video(v)) = parse(&long) else {
            panic!()
        };
        assert_eq!(v.title.chars().count(), 81);
    }

    #[test]
    fn instagram_carousel_is_rejected() {
        let json = r#"{"_type": "playlist", "id": "C8xYz12AbCd", "extractor_key": "Instagram",
            "entries": [{"id": "1"}, {"id": "2"}]}"#;
        assert!(parse(json).unwrap_err().contains("several"));
    }

    #[test]
    fn tiktok_video() {
        let json = r#"{"id": "6742501081818877190", "extractor_key": "TikTok",
            "title": "Tag 1 Friend reverse this Video and look what happens 🤩😱 @skyandtami ...",
            "description": "Tag 1 Friend reverse this Video and look what happens 🤩😱 @skyandtami  are the Best 🤩 #patrox #sotiktok #viral",
            "uploader": "patrox", "channel": "patroX", "duration": 27,
            "thumbnail": "https://p16-common-sign.tiktokcdn-eu.com/x~tplv-tiktokx-origin.image?x-expires=1",
            "webpage_url": "https://www.tiktok.com/@patroxofficial/video/6742501081818877190"}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert!(v.fixed_title);
        // From the caption, not yt-dlp's title that ends in "...".
        assert_eq!(
            v.title,
            "Tag 1 Friend reverse this Video and look what happens 🤩😱 @skyandtami  are the Be…"
        );
        assert_eq!(v.channel.as_deref(), Some("patroX"));
        assert_eq!(v.duration, Some(27.0));
        assert_eq!(
            v.url,
            "https://www.tiktok.com/@patroxofficial/video/6742501081818877190"
        );
        assert!(v.thumbnail.unwrap().contains("tiktokcdn"));

        let json =
            r#"{"id": "6742501081818877190", "extractor_key": "TikTok", "uploader": "patrox"}"#;
        let Ok(Preview::Video(v)) = parse(json) else {
            panic!()
        };
        assert_eq!(v.title, "TikTok by patrox");
        assert_eq!(v.url, "https://www.tiktok.com/@/video/6742501081818877190");
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
