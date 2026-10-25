//! Turning yt-dlp/ffmpeg failure output into messages a person can act on.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserError {
    pub message: String,
    pub hint: Option<String>,
    /// Raw tool output (last lines), for a "details" view.
    pub detail: String,
}

impl UserError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
            detail: String::new(),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Does the hint suggest updating yt-dlp?
    pub fn suggests_update(&self) -> bool {
        self.hint
            .as_deref()
            .is_some_and(|h| h.contains(UPDATE_HINT))
    }
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(h) = &self.hint {
            write!(f, " {h}")?;
        }
        Ok(())
    }
}

pub const UPDATE_HINT: &str = "Try \"Update yt-dlp\"";

struct Rule {
    needles: &'static [&'static str],
    message: &'static str,
    hint: Option<&'static str>,
}

const UPDATE: Option<&str> =
    Some("Try \"Update yt-dlp\": YouTube changes often and older versions break.");

const RULES: &[Rule] = &[
    Rule {
        needles: &[
            "no space left on device",
            "[errno 28]",
            "not enough space on the disk",
            "winerror 112",
            "os error 112",
        ],
        message: "The disk is full.",
        hint: Some("Free up space or choose another output folder."),
    },
    // Signed out, YouTube reports private playlists as non-existent. Must come
    // before the generic "does not exist" video rule.
    Rule {
        needles: &[
            "playlist does not exist",
            "this playlist is private",
            "playlist is private",
            "playlist is unavailable",
        ],
        message: "This playlist is private (or doesn't exist). YouTube only shows private playlists to their owner, and YtLocal doesn't sign in.",
        hint: Some(
            "In YouTube, set the playlist's visibility to Unlisted, then try again. Or pick \"This video\".",
        ),
    },
    Rule {
        needles: &["private video"],
        message: "This video is private.",
        hint: None,
    },
    Rule {
        needles: &[
            "confirm your age",
            "age-restricted",
            "age restricted",
            "inappropriate for some users",
        ],
        message: "This video is age-restricted and needs a signed-in account, which YtLocal doesn't use.",
        hint: None,
    },
    Rule {
        needles: &[
            "not available in your country",
            "not made this video available in your country",
            "geo restrict",
            "geo-restrict",
            "blocked it in your country",
        ],
        message: "This video is blocked in your region.",
        hint: None,
    },
    Rule {
        needles: &[
            "members-only",
            "join this channel",
            "available to this channel's members",
        ],
        message: "This video is for channel members only.",
        hint: None,
    },
    Rule {
        needles: &[
            "premieres in",
            "live event will begin",
            "this live event",
            "is not live yet",
            "is_upcoming",
        ],
        message: "This is an upcoming premiere or live stream; it can't be downloaded yet.",
        hint: None,
    },
    Rule {
        needles: &[
            "has been removed",
            "account associated with this video has been terminated",
            "no longer available",
            "video unavailable",
            "this video is unavailable",
            "does not exist",
        ],
        message: "This video is unavailable (removed, deleted or never existed).",
        hint: None,
    },
    Rule {
        needles: &["not a bot"],
        message: "YouTube wants to confirm you're not a bot.",
        hint: Some("Wait a while and try again. Try \"Update yt-dlp\" too."),
    },
    Rule {
        needles: &[
            "getaddrinfo failed",
            "failed to resolve",
            "name or service not known",
            "network is unreachable",
            "no route to host",
            "errno 11001",
            "errno 11002",
            "connection refused",
            "connection reset",
            "timed out",
            "remote end closed connection",
            "unable to connect",
            "connectionerror",
        ],
        message: "Couldn't reach YouTube. Check your internet connection.",
        hint: None,
    },
    Rule {
        needles: &["is not a valid url", "unsupported url"],
        message: "That link isn't a valid YouTube video or playlist.",
        hint: None,
    },
    Rule {
        needles: &[
            "ffmpeg not found",
            "ffprobe not found",
            "ffmpeg is not installed",
            "ffprobe/avprobe and ffmpeg/avconv not found",
        ],
        message: "ffmpeg is missing.",
        hint: Some("Download it from the Tools panel."),
    },
    Rule {
        needles: &[
            "http error 403",
            "http error 429",
            "nsig",
            "signature extraction",
            "unable to extract",
            "requested format is not available",
            "please report this issue",
            "extractorerror",
            "unable to download api page",
            "unable to download webpage",
            "po token",
            "sabr",
        ],
        message: "YouTube refused the download (extractor error).",
        hint: UPDATE,
    },
    Rule {
        needles: &[
            "postprocessing:",
            "conversion failed",
            "error opening output",
            "invalid data found",
        ],
        message: "Converting the file with ffmpeg failed.",
        hint: None,
    },
];

/// Strip "ERROR: [youtube] abc123: " style prefixes.
fn clean_error_line(line: &str) -> String {
    let mut s = line.trim();
    s = s.strip_prefix("ERROR:").unwrap_or(s).trim_start();
    if s.starts_with('[')
        && let Some(i) = s.find("] ")
    {
        s = &s[i + 2..];
        // "abc123def45: message"
        if let Some((id, rest)) = s.split_once(": ")
            && !id.contains(' ')
        {
            s = rest;
        }
    }
    s.to_string()
}

/// Classify the stderr of a failed yt-dlp run.
pub fn classify(stderr_lines: &[String]) -> UserError {
    let detail = stderr_lines
        .iter()
        .rev()
        .take(15)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let errors: Vec<&String> = stderr_lines
        .iter()
        .filter(|l| l.trim_start().starts_with("ERROR"))
        .collect();
    // Prefer ERROR lines; fall back to everything (python tracebacks etc.).
    let haystack = if errors.is_empty() {
        stderr_lines.join("\n")
    } else {
        errors
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
    .to_lowercase();

    for rule in RULES {
        if rule.needles.iter().any(|n| haystack.contains(n)) {
            let mut e = UserError::new(rule.message).with_detail(detail);
            if let Some(h) = rule.hint {
                e = e.with_hint(h);
            }
            return e;
        }
    }

    let message = errors
        .last()
        .map(|l| clean_error_line(l))
        .or_else(|| {
            stderr_lines
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_else(|| "yt-dlp failed without an error message.".to_string());
    UserError::new(message)
        .with_hint(UPDATE.unwrap())
        .with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(lines: &[&str]) -> UserError {
        classify(&lines.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn known_failures() {
        assert_eq!(c(&["ERROR: [youtube] abc: Private video. Sign in if you've been granted access to this video"]).message, "This video is private.");
        assert!(c(&["ERROR: [youtube] abc: Sign in to confirm your age. This video may be inappropriate for some users."]).message.contains("age-restricted"));
        assert!(c(&["ERROR: [youtube] abc: The uploader has not made this video available in your country"]).message.contains("region"));
        assert!(c(&["ERROR: [youtube] abc: Video unavailable. This video has been removed by the uploader"]).message.contains("unavailable"));
        assert!(
            c(&["ERROR: [youtube] abc: Video unavailable"])
                .message
                .contains("unavailable")
        );
        assert!(
            c(&["ERROR: [Errno 28] No space left on device"])
                .message
                .contains("disk is full")
        );
        assert!(
            c(&[
                "ERROR: unable to write data: [WinError 112] There is not enough space on the disk"
            ])
            .message
            .contains("disk is full")
        );
        assert!(
            c(&["ERROR: [generic] 'foo' is not a valid URL."])
                .message
                .contains("valid")
        );
    }

    #[test]
    fn network_errors() {
        let e = c(&[
            "WARNING: something",
            "ERROR: [youtube] abc: Unable to download API page: <urlopen error [Errno 11001] getaddrinfo failed> (caused by URLError(gaierror(11001, 'getaddrinfo failed')))",
        ]);
        assert!(e.message.contains("internet"), "{e:?}");
        assert!(!e.suggests_update());
    }

    #[test]
    fn extractor_errors_suggest_update() {
        let e = c(&[
            "ERROR: [youtube] abc: Unable to extract nsig function code; please report this issue on https://github.com/yt-dlp/yt-dlp/issues",
        ]);
        assert!(e.suggests_update());
        let e = c(&["ERROR: unable to download video data: HTTP Error 403: Forbidden"]);
        assert!(e.suggests_update());
        assert!(e.detail.contains("403"));
    }

    #[test]
    fn private_playlist_is_not_called_a_video() {
        let e = c(&[
            "WARNING: [youtube:tab] YouTube said: The playlist does not exist.",
            "ERROR: [youtube:tab] PLy5RfAaAn0K_hy8aOWqQrkd39nJRiDtBi: YouTube said: The playlist does not exist.",
        ]);
        assert!(e.message.contains("playlist is private"), "{e:?}");
        assert!(e.hint.as_deref().unwrap().contains("Unlisted"));
        assert!(!e.suggests_update());
    }

    #[test]
    fn unknown_error_uses_cleaned_line() {
        let e = c(&["ERROR: [youtube] dPZTh2NKTm4: Something brand new happened"]);
        assert_eq!(e.message, "Something brand new happened");
        assert!(e.suggests_update());
        let e = c(&[]);
        assert!(e.message.contains("without an error"));
    }
}
