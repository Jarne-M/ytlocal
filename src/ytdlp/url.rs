//! Recognizing YouTube, Instagram and TikTok links. We only accept URLs we
//! understand and turn them into canonical forms, so yt-dlp always gets a
//! clean single-video or playlist URL.

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
    /// An Instagram reel (or video post), by its shortcode.
    Instagram {
        code: String,
    },
    /// A TikTok video: `id` is the numeric video id, or the code of a short
    /// share link (vm.tiktok.com/...), which yt-dlp resolves itself.
    TikTok {
        id: String,
        url: String,
    },
}

impl Link {
    pub fn video_url(id: &str) -> String {
        format!("https://www.youtube.com/watch?v={id}")
    }

    pub fn playlist_url(list: &str) -> String {
        format!("https://www.youtube.com/playlist?list={list}")
    }

    pub fn instagram_url(code: &str) -> String {
        format!("https://www.instagram.com/reel/{code}/")
    }

    /// Thumbnail that always exists for a YouTube video id (320x180 JPEG).
    pub fn thumbnail_url(id: &str) -> String {
        format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    NotAUrl,
    /// Not a site we support.
    UnsupportedSite,
    /// A YouTube URL we don't handle (channel pages, search, ...).
    Unsupported,
    /// An Instagram URL that isn't a reel/post (profile, stories, ...).
    UnsupportedInstagram,
    /// A TikTok URL that isn't a single video (profile, photo post, live, ...).
    UnsupportedTikTok,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkError::NotAUrl => "This doesn't look like a link.",
            LinkError::UnsupportedSite => {
                "Only YouTube, Instagram and TikTok links are supported."
            }
            LinkError::UnsupportedTikTok => {
                "Unsupported TikTok link. Use a link to one video (not a profile, photo post or live)."
            }
            LinkError::Unsupported => {
                "Unsupported YouTube link. Use a video, Shorts or playlist link (not a channel or search page)."
            }
            LinkError::UnsupportedInstagram => {
                "Unsupported Instagram link. Use a reel or video post link (not a profile or story)."
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

fn is_instagram_host(host: &str) -> bool {
    let host = host.trim_start_matches("www.");
    matches!(host, "instagram.com" | "m.instagram.com" | "instagr.am")
}

fn is_instagram_code(s: &str) -> bool {
    (5..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `/reel/CODE`, `/reels/CODE`, `/p/CODE`, `/tv/CODE`, also behind a
/// username (`/someone/reel/CODE`). `/reels/audio/...` is a sound page.
fn parse_instagram(segments: &[&str]) -> Result<Link, LinkError> {
    let code = match segments {
        ["reel" | "reels" | "p" | "tv", code, ..] => code,
        [_, "reel" | "reels" | "p" | "tv", code, ..] => code,
        _ => return Err(LinkError::UnsupportedInstagram),
    };
    if *code == "audio" || !is_instagram_code(code) {
        return Err(LinkError::UnsupportedInstagram);
    }
    Ok(Link::Instagram {
        code: code.to_string(),
    })
}

fn is_tiktok_host(host: &str) -> bool {
    let host = host.trim_start_matches("www.");
    matches!(
        host,
        "tiktok.com" | "m.tiktok.com" | "vm.tiktok.com" | "vt.tiktok.com"
    )
}

fn is_digits(s: &str) -> bool {
    (5..=30).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_short_code(s: &str) -> bool {
    (4..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `/@user/video/ID`, `/embed/ID`, `/embed/v2/ID`, `m.tiktok.com/v/ID.html`,
/// and short share links `vm.tiktok.com/CODE`, `vt.tiktok.com/CODE`, `/t/CODE`.
fn parse_tiktok(host: &str, segments: &[&str]) -> Result<Link, LinkError> {
    let host = host.trim_start_matches("www.");
    if host == "vm.tiktok.com" || host == "vt.tiktok.com" {
        return match segments {
            [code, ..] if is_short_code(code) => Ok(Link::TikTok {
                id: code.to_string(),
                url: format!("https://{host}/{code}/"),
            }),
            _ => Err(LinkError::UnsupportedTikTok),
        };
    }
    let video = |id: &str, user: &str| {
        if !is_digits(id) {
            return Err(LinkError::UnsupportedTikTok);
        }
        Ok(Link::TikTok {
            id: id.to_string(),
            url: format!("https://www.tiktok.com/{user}/video/{id}"),
        })
    };
    match segments {
        ["t", code, ..] if is_short_code(code) => Ok(Link::TikTok {
            id: code.to_string(),
            url: format!("https://www.tiktok.com/t/{code}/"),
        }),
        [user, "video", id, ..] if user.starts_with('@') => {
            let ok = user[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-');
            video(id, if ok { user } else { "@" })
        }
        ["embed", "v2", id, ..] | ["embed", id, ..] => video(id, "@"),
        ["v", file, ..] => video(file.trim_end_matches(".html"), "@"),
        _ => Err(LinkError::UnsupportedTikTok),
    }
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
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    if is_instagram_host(&host) {
        return parse_instagram(&segments);
    }
    if is_tiktok_host(&host) {
        return parse_tiktok(&host, &segments);
    }
    if !is_youtube_host(&host) {
        return Err(LinkError::UnsupportedSite);
    }

    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let list = query("list").filter(|l| is_list_id(l));

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

/// First supported link in arbitrary text (used for clipboard auto-detect).
pub fn find_link(text: &str) -> Option<String> {
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
        assert_eq!(
            parse("https://vimeo.com/12345"),
            Err(LinkError::UnsupportedSite)
        );
        assert_eq!(
            parse("https://notyoutube.com/watch?v=dPZTh2NKTm4"),
            Err(LinkError::UnsupportedSite)
        );
        assert_eq!(
            parse("https://notinstagram.com/reel/C8xYz12AbCd/"),
            Err(LinkError::UnsupportedSite)
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
    fn instagram() {
        let reel = |c: &str| Ok(Link::Instagram { code: c.into() });
        assert_eq!(
            parse("https://www.instagram.com/reel/C8xYz12AbCd/?igsh=MWZ4c2x5"),
            reel("C8xYz12AbCd")
        );
        assert_eq!(
            parse("instagram.com/reels/C8xYz12AbCd"),
            reel("C8xYz12AbCd")
        );
        assert_eq!(
            parse("https://www.instagram.com/p/C8xYz12AbCd/?utm_source=ig_web_copy_link"),
            reel("C8xYz12AbCd")
        );
        assert_eq!(
            parse("https://www.instagram.com/someone/reel/C8xYz12AbCd/"),
            reel("C8xYz12AbCd")
        );
        assert_eq!(
            parse("https://m.instagram.com/tv/C8xYz12AbCd"),
            reel("C8xYz12AbCd")
        );
        assert_eq!(
            Link::instagram_url("C8xYz12AbCd"),
            "https://www.instagram.com/reel/C8xYz12AbCd/"
        );
        for bad in [
            "https://www.instagram.com/someone/",
            "https://www.instagram.com/stories/someone/123456789/",
            "https://www.instagram.com/reels/audio/123456789/",
            "https://www.instagram.com/reel/",
            "https://www.instagram.com/",
        ] {
            assert_eq!(parse(bad), Err(LinkError::UnsupportedInstagram), "{bad}");
        }
    }

    #[test]
    fn tiktok() {
        let tt = |id: &str, url: &str| {
            Ok(Link::TikTok {
                id: id.into(),
                url: url.into(),
            })
        };
        assert_eq!(
            parse(
                "https://www.tiktok.com/@patroxofficial/video/6742501081818877190?is_from_webapp=1&sender_device=pc"
            ),
            tt(
                "6742501081818877190",
                "https://www.tiktok.com/@patroxofficial/video/6742501081818877190"
            )
        );
        assert_eq!(
            parse("tiktok.com/@some.one_1/video/6742501081818877190"),
            tt(
                "6742501081818877190",
                "https://www.tiktok.com/@some.one_1/video/6742501081818877190"
            )
        );
        assert_eq!(
            parse("https://m.tiktok.com/v/6742501081818877190.html"),
            tt(
                "6742501081818877190",
                "https://www.tiktok.com/@/video/6742501081818877190"
            )
        );
        assert_eq!(
            parse("https://www.tiktok.com/embed/v2/6742501081818877190"),
            tt(
                "6742501081818877190",
                "https://www.tiktok.com/@/video/6742501081818877190"
            )
        );
        assert_eq!(
            parse("https://vm.tiktok.com/ZMabc123/"),
            tt("ZMabc123", "https://vm.tiktok.com/ZMabc123/")
        );
        assert_eq!(
            parse("vt.tiktok.com/ZSxyz789"),
            tt("ZSxyz789", "https://vt.tiktok.com/ZSxyz789/")
        );
        assert_eq!(
            parse("https://www.tiktok.com/t/ZTRabc12/"),
            tt("ZTRabc12", "https://www.tiktok.com/t/ZTRabc12/")
        );
        for bad in [
            "https://www.tiktok.com/@patroxofficial",
            "https://www.tiktok.com/@patroxofficial/photo/7300000000000000000",
            "https://www.tiktok.com/@patroxofficial/live",
            "https://www.tiktok.com/@patroxofficial/video/notanumber",
            "https://www.tiktok.com/foryou",
            "https://vm.tiktok.com/",
        ] {
            assert_eq!(parse(bad), Err(LinkError::UnsupportedTikTok), "{bad}");
        }
        assert_eq!(
            parse("https://nottiktok.com/@a/video/6742501081818877190"),
            Err(LinkError::UnsupportedSite)
        );
    }

    #[test]
    fn splitting_and_finding() {
        let text = "https://youtu.be/XAUoA7q_xnM\r\n\n  https://www.youtube.com/watch?v=dPZTh2NKTm4 , extra";
        assert_eq!(split_input(text).len(), 3);
        assert_eq!(
            find_link("check this out: https://youtu.be/XAUoA7q_xnM !").as_deref(),
            Some("https://youtu.be/XAUoA7q_xnM")
        );
        assert_eq!(
            find_link("kijk https://www.instagram.com/reel/C8xYz12AbCd/").as_deref(),
            Some("https://www.instagram.com/reel/C8xYz12AbCd/")
        );
        assert_eq!(find_link("nothing here"), None);
    }
}
