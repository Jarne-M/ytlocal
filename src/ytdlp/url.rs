//! Recognizing YouTube links. We only accept URLs we understand and turn them
//! into canonical forms, so yt-dlp always gets a clean single-video or
//! playlist URL.

use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Video {
        id: String,
    },
    Playlist {
        list: String,
    },
    /// A watch URL that also carries `list=`: the user picks video or whole playlist.
    VideoInPlaylist {
        id: String,
        list: String,
    },
}

impl Link {
    pub fn video_url(id: &str) -> String {
        format!("https://www.youtube.com/watch?v={id}")
    }

    pub fn playlist_url(list: &str) -> String {
        format!("https://www.youtube.com/playlist?list={list}")
    }

    /// Thumbnail that always exists for a video id (320x180 JPEG).
    pub fn thumbnail_url(id: &str) -> String {
        format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    NotAUrl,
    NotYouTube,
    /// A YouTube URL we don't handle (channel pages, search, ...).
    Unsupported,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkError::NotAUrl => "This doesn't look like a link.",
            LinkError::NotYouTube => "Only YouTube links are supported.",
            LinkError::Unsupported => {
                "Unsupported YouTube link. Use a video, Shorts or playlist link (not a channel or search page)."
            }
        })
    }
}

fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_list_id(s: &str) -> bool {
    (2..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn is_youtube_host(host: &str) -> bool {
    let host = host.trim_start_matches("www.");
    matches!(
        host,
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtube-nocookie.com" | "youtu.be"
    )
}

/// Parse one link. Accepts missing schemes ("youtu.be/abc...") and
/// surrounding whitespace or angle brackets.
pub fn parse(input: &str) -> Result<Link, LinkError> {
    let s = input
        .trim()
        .trim_matches(|c| c == '<' || c == '>' || c == '"');
    if s.is_empty() || s.contains(char::is_whitespace) {
        return Err(LinkError::NotAUrl);
    }
    let with_scheme = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else if s.contains('.') && s.contains('/') {
        format!("https://{s}")
    } else {
        return Err(LinkError::NotAUrl);
    };
    let url = Url::parse(&with_scheme).map_err(|_| LinkError::NotAUrl)?;
    let host = url
        .host_str()
        .ok_or(LinkError::NotAUrl)?
        .to_ascii_lowercase();
    if !is_youtube_host(&host) {
        return Err(LinkError::NotYouTube);
    }

    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let list = query("list").filter(|l| is_list_id(l));
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    let video_id = if host == "youtu.be" {
        segments.first().map(|s| s.to_string())
    } else {
        match segments.as_slice() {
            ["watch"] => query("v"),
            ["shorts" | "live" | "embed" | "v" | "e", id, ..] => Some(id.to_string()),
            ["playlist"] => None,
            _ => return Err(LinkError::Unsupported),
        }
    };

    match (video_id, list) {
        (Some(id), _) if !is_video_id(&id) => Err(LinkError::Unsupported),
        (Some(id), Some(list)) => Ok(Link::VideoInPlaylist { id, list }),
        (Some(id), None) => Ok(Link::Video { id }),
        (None, Some(list)) => Ok(Link::Playlist { list }),
        (None, None) => Err(LinkError::Unsupported),
    }
}

/// Split pasted text into candidate links (one per line, or separated by spaces).
pub fn split_input(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// First YouTube link in arbitrary text (used for clipboard auto-detect).
pub fn find_youtube_link(text: &str) -> Option<String> {
    split_input(text).into_iter().find(|s| parse(s).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str) -> Result<Link, LinkError> {
        Ok(Link::Video { id: id.into() })
    }

    #[test]
    fn watch_urls() {
        assert_eq!(
            parse("https://www.youtube.com/watch?v=dPZTh2NKTm4"),
            video("dPZTh2NKTm4")
        );
        assert_eq!(
            parse("http://youtube.com/watch?v=dPZTh2NKTm4&t=42s"),
            video("dPZTh2NKTm4")
        );
        assert_eq!(
            parse("https://m.youtube.com/watch?feature=share&v=dPZTh2NKTm4"),
            video("dPZTh2NKTm4")
        );
        assert_eq!(
            parse("  www.youtube.com/watch?v=dPZTh2NKTm4  "),
            video("dPZTh2NKTm4")
        );
        assert_eq!(
            parse("<https://www.youtube.com/watch?v=dPZTh2NKTm4>"),
            video("dPZTh2NKTm4")
        );
    }

    #[test]
    fn short_forms() {
        assert_eq!(parse("https://youtu.be/XAUoA7q_xnM"), video("XAUoA7q_xnM"));
        assert_eq!(
            parse("youtu.be/XAUoA7q_xnM?si=abcdef&t=3"),
            video("XAUoA7q_xnM")
        );
        assert_eq!(
            parse("https://www.youtube.com/shorts/XAUoA7q_xnM"),
            video("XAUoA7q_xnM")
        );
        assert_eq!(
            parse("https://youtube.com/shorts/XAUoA7q_xnM?feature=share"),
            video("XAUoA7q_xnM")
        );
        assert_eq!(
            parse("https://www.youtube.com/live/XAUoA7q_xnM"),
            video("XAUoA7q_xnM")
        );
        assert_eq!(
            parse("https://www.youtube-nocookie.com/embed/XAUoA7q_xnM"),
            video("XAUoA7q_xnM")
        );
    }

    #[test]
    fn music_youtube() {
        assert_eq!(
            parse("https://music.youtube.com/watch?v=XAUoA7q_xnM&feature=share"),
            video("XAUoA7q_xnM")
        );
        assert_eq!(
            parse("https://music.youtube.com/playlist?list=OLAK5uy_kXyz123"),
            Ok(Link::Playlist {
                list: "OLAK5uy_kXyz123".into()
            })
        );
    }

    #[test]
    fn playlists() {
        assert_eq!(
            parse("https://www.youtube.com/playlist?list=PLBCF2DAC6FFB574DE"),
            Ok(Link::Playlist {
                list: "PLBCF2DAC6FFB574DE".into()
            })
        );
        assert_eq!(
            parse("https://www.youtube.com/watch?v=dPZTh2NKTm4&list=PLBCF2DAC6FFB574DE&index=3"),
            Ok(Link::VideoInPlaylist {
                id: "dPZTh2NKTm4".into(),
                list: "PLBCF2DAC6FFB574DE".into()
            })
        );
        assert_eq!(
            parse("https://youtu.be/dPZTh2NKTm4?list=PLBCF2DAC6FFB574DE"),
            Ok(Link::VideoInPlaylist {
                id: "dPZTh2NKTm4".into(),
                list: "PLBCF2DAC6FFB574DE".into()
            })
        );
    }

    #[test]
    fn rejects() {
        assert_eq!(parse("hello"), Err(LinkError::NotAUrl));
        assert_eq!(parse(""), Err(LinkError::NotAUrl));
        assert_eq!(parse("https://vimeo.com/12345"), Err(LinkError::NotYouTube));
        assert_eq!(
            parse("https://notyoutube.com/watch?v=dPZTh2NKTm4"),
            Err(LinkError::NotYouTube)
        );
        assert_eq!(
            parse("https://www.youtube.com/@SomeChannel"),
            Err(LinkError::Unsupported)
        );
        assert_eq!(
            parse("https://www.youtube.com/results?search_query=x"),
            Err(LinkError::Unsupported)
        );
        assert_eq!(
            parse("https://www.youtube.com/watch?v=short"),
            Err(LinkError::Unsupported)
        );
        assert_eq!(
            parse("https://www.youtube.com/watch"),
            Err(LinkError::Unsupported)
        );
        assert_eq!(
            parse("https://www.youtube.com/playlist"),
            Err(LinkError::Unsupported)
        );
    }

    #[test]
    fn splitting_and_finding() {
        let text = "https://youtu.be/XAUoA7q_xnM\r\n\n  https://www.youtube.com/watch?v=dPZTh2NKTm4 , extra";
        assert_eq!(split_input(text).len(), 3);
        assert_eq!(
            find_youtube_link("check this out: https://youtu.be/XAUoA7q_xnM !").as_deref(),
            Some("https://youtu.be/XAUoA7q_xnM")
        );
        assert_eq!(find_youtube_link("nothing here"), None);
    }
}
