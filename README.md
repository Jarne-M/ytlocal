# YtLocal

A local YouTube → MP3/MP4 converter for Windows, written in Rust (eframe/egui).
It drives [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [ffmpeg](https://ffmpeg.org/) as child processes.
Everything runs on your machine: no accounts, no cloud, no telemetry, no ads.

For personal use: download things you're allowed to keep offline, and respect
the creator's license and YouTube's terms.

## Download

Get the newest `YtLocal.exe` (or the zip) from the **[Releases page](../../releases/latest)**
and double-click it. You don't need to install anything. On first start, click **Download** next
to yt-dlp, FFmpeg and Deno in the Tools panel (once, about a minute). If Windows says "Windows
protected your PC", click *More info → Run anyway*; the exe isn't code-signed.

The version is shown at the top of the window; compare it with the latest release to see
whether there's a newer one.

## Features

- Paste one or more links (videos, Shorts, `youtu.be`, `music.youtube.com`, playlists), or
  start the app with links: `ytlocal.exe <link> <link>…`.
- Preview before downloading: thumbnail, title, channel and duration. Playlists show every item
  with checkboxes (All / None). A watch link carrying `list=` lets you pick *this video* or the
  *whole playlist*. Private, age-restricted, region-locked, removed and live videos get a clear message.
- Audio: MP3 (128/192/256/320 kbps), M4A (AAC), Opus, WAV, FLAC. M4A and Opus are stream-copied
  when YouTube already has that codec, so there's no quality loss.
- Video: MP4 at Best / 1080p / 720p / 480p / 360p. It falls back to the nearest lower resolution,
  and the result is always **H.264 + AAC in MP4**: anything else (VP9, AV1, Opus, WebM) is
  re-encoded or remuxed so the file plays everywhere.
- Tags (title, artist = channel, album = playlist name) and the thumbnail as cover art (not for WAV).
- Filename template (default `%(title)s.%(ext)s`). Names are made Windows-safe and
  length-limited, and existing files are never overwritten (keep both as `name (1).mp3`, or skip).
- Queue with configurable parallel downloads (default 2). Each item shows progress, speed and ETA,
  with cancel, retry, open file and show in folder. Cancel all, clear finished.
  Cancelling kills the whole yt-dlp/ffmpeg process tree and deletes partial files.
- Extras: trim (start/end), loudness normalization (EBU R128, −16 LUFS), subtitles embedded
  in MP4, speed limit.
- Settings, the last-used format, window size and the last 50 downloads are remembered.
- A Windows notification and an in-app message when downloads finish.

## Build

Install Rust from [rustup.rs](https://rustup.rs) (MSVC or GNU toolchain), then:

```powershell
cargo build --release       # -> target\release\ytlocal.exe (single exe, ~17 MB)
cargo run                   # or run a debug build directly
```

`env.ps1` is only for a portable toolchain in `..\.toolchain` (no system-wide Rust).
Dot-source it first (`. .\env.ps1`) if that's your setup.

## Contributing

1. Fork the repo (or, as a collaborator, create a branch), make your change, and run
   `cargo test` and `cargo clippy --all-targets`.
2. Open a pull request. GitHub Actions builds and tests it on Windows automatically.

The [Architecture](#architecture) section below shows where things live.

## Releasing a new version

1. Bump `version` in `Cargo.toml` (e.g. `0.2.0`) and commit.
2. Tag and push:
   ```powershell
   git tag v0.2.0
   git push origin main --tags
   ```
3. GitHub Actions builds the exe and publishes a Release with `YtLocal.exe` and
   `YtLocal-windows.zip`. The Releases page always shows the newest one.

## First run

The app looks for `yt-dlp.exe`, `ffmpeg.exe`/`ffprobe.exe` and `deno.exe` in this order:
1. next to `ytlocal.exe`
2. `%APPDATA%\YtLocal\bin`
3. `PATH`

Missing tools can be downloaded with one click (**Tools** button, top right):

| Tool | Source | Verified by |
|---|---|---|
| yt-dlp | github.com/yt-dlp/yt-dlp, latest release | `SHA2-256SUMS` + running `--version` |
| ffmpeg + ffprobe | github.com/BtbN/FFmpeg-Builds, `ffmpeg-master-latest-win64-gpl.zip` | `checksums.sha256` + `-version` |
| deno (optional) | github.com/denoland/deno, latest release | `.sha256sum` + `--version` |

**Why deno?** Current yt-dlp needs a JavaScript runtime to solve YouTube's challenges. Without it,
some formats may be missing. It's optional but recommended.

**Update yt-dlp** runs `yt-dlp -U`, or re-downloads it if that's not possible (for example a pip
install on PATH). YouTube changes often, so update when downloads start failing. Errors that look
like an extractor problem show an "Update yt-dlp" button. A corrupt binary is detected (it fails to
run) and can be reinstalled.

## Files

| What | Where |
|---|---|
| Settings | `%APPDATA%\YtLocal\config.json` |
| History (last 50) | `%APPDATA%\YtLocal\history.json` |
| Downloaded tools | `%APPDATA%\YtLocal\bin` |
| In-progress downloads | `%LOCALAPPDATA%\YtLocal\tmp\job-*` (deleted after every job; leftovers from a crash are removed on a later start) |

A corrupt `config.json` is renamed to `config.json.bad` and defaults are used.

## Tests

```powershell
cargo test                                         # unit tests + offline settings GUI test
cargo test --test gui_e2e -- --ignored --nocapture # real UI + real downloads (needs network and tools)
cargo clippy --all-targets
```

Unit tests cover URL parsing, filename sanitizing, progress parsing, command building, error
classification, the scheduler, the MP4 compatibility plan, config/history persistence and moving
files into place.

`tests/gui_e2e.rs` drives the actual UI with `egui_kittest`. It types links, fetches previews,
picks playlist items, downloads, and checks the files and history. Set `YTLOCAL_SHOTS=<folder>`
to also save rendered frames of each step. `YTLOCAL_DATA_DIR` keeps test settings away from your
real ones.

`examples/headless.rs` runs the same backend without a window:

```powershell
cargo run --example headless -- tools
cargo run --example headless -- install ytdlp|ffmpeg|deno
cargo run --example headless -- preview <url> [playlist]
cargo run --example headless -- download <out-dir> mp3-320|m4a|opus|wav|flac|mp4-best|mp4-1080|… <url>… `
    [--normalize] [--trim 0:10-0:20] [--subs] [--album NAME] [--skip] [--rate KiB] [--h264-any] [--cancel-after SECS]
```

## Architecture

```
src/
  main.rs        window setup, loads config, command-line links
  backend.rs     request/event bridge: UI -> tokio tasks -> UI (wakes egui on every event)
  binaries/      locate, verify (--version), download (+SHA-256), update the tools
  ytdlp/         url.rs (link parsing), command.rs (yt-dlp args), progress.rs (output parsing),
                 metadata.rs (-J previews), errors.rs (user-facing messages), filename.rs
  queue/         Scheduler (concurrency), runner.rs (one job: yt-dlp -> MP4 fix-up -> move into place)
  ffmpeg.rs      ffprobe + H.264/AAC conversion plan and runner
  config/        config.json and history.json
  process.rs     CREATE_NO_WINDOW, UTF-8 output, Job Object process-tree kill
  notify.rs      Windows toasts
  ui/            egui panels: input/previews, options, queue, history, settings, tools
```

- The GUI thread never waits on processes, the network or the disk. Everything runs on a tokio
  runtime and reports back over channels. Settings are saved on a background task (debounced),
  and once more on close.
- yt-dlp output is parsed from machine-readable lines it prints for us (`--progress-template` with
  `%(progress)j`, `--print` with a `YTL*` prefix). Any other line is ignored, so new yt-dlp messages
  can't break parsing.
- Each job runs in its own temp folder and is moved into the output folder only when complete.
  A cancel or crash never leaves partial files next to your music.

## Troubleshooting

- **"YouTube refused the download (extractor error)" / HTTP 403**: click *Update yt-dlp*.
  Installing deno also helps.
- **Age-restricted videos**: need a signed-in account; YtLocal deliberately doesn't use one.
- **No notification**: check Windows Focus Assist / Do Not Disturb. The in-app message always shows.
- **"Best" quality is 1080p**: that's the fastest option (YouTube's own H.264 streams). For 1440p/4K,
  pick *Highest* under Settings → Best video quality. It's re-encoded to H.264, which is slow.
