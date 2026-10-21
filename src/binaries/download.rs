//! One-click install of the tools from their official release pages, with
//! SHA-256 verification against the checksums each project publishes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::{Package, Tool, probe};
use crate::paths;

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YTDLP_SUMS: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const FFMPEG_ZIP: &str = "ffmpeg-master-latest-win64-gpl.zip";
const FFMPEG_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
const FFMPEG_SUMS: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/checksums.sha256";
const DENO_ZIP: &str = "deno-x86_64-pc-windows-msvc.zip";
const DENO_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip";
const DENO_SUMS: &str = "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip.sha256sum";

#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("YtLocal/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .expect("http client")
}

/// Find the SHA-256 for `file_name` in a checksum file. Handles the usual
/// `<hash>  <name>` lists as well as PowerShell `Get-FileHash` output.
pub fn find_sha256(text: &str, file_name: &str) -> Option<String> {
    fn hash_in(line: &str) -> Option<String> {
        line.split(|c: char| !c.is_ascii_hexdigit())
            .find(|w| w.len() == 64)
            .map(|w| w.to_ascii_lowercase())
    }
    // Exact name, optionally prefixed by a path or sha256sum's binary-mode '*'.
    fn names_file(line: &str, file_name: &str) -> bool {
        line.split_whitespace().any(|tok| {
            let base = tok.rsplit(['\\', '/']).next().unwrap_or(tok);
            base.trim_start_matches('*') == file_name
        })
    }
    for line in text.lines() {
        if names_file(line, file_name)
            && let Some(h) = hash_in(line)
        {
            return Some(h);
        }
    }
    // Single-file checksum (hash and name on separate lines).
    let mut all: Vec<String> = text.lines().filter_map(hash_in).collect();
    all.dedup();
    if all.len() == 1 && text.lines().any(|l| names_file(l, file_name)) {
        return all.pop();
    }
    None
}

/// Staging name for a freshly downloaded exe; keeps `.exe` so it can be run for verification.
fn staging_name(exe: &str) -> String {
    format!("{}.new.exe", exe.trim_end_matches(".exe"))
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let resp = client.get(url).send().await.map_err(net_err)?;
    let resp = resp.error_for_status()?;
    Ok(resp.text().await?)
}

fn net_err(e: reqwest::Error) -> anyhow::Error {
    if e.is_connect() || e.is_timeout() {
        anyhow!("Could not reach GitHub. Check your internet connection. ({e})")
    } else {
        anyhow!(e)
    }
}

/// Stream `url` to `dest`, returning the lowercase hex SHA-256 of the body.
async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    stage: &str,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> anyhow::Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(net_err)?
        .error_for_status()?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(net_err)?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| format!("writing {}", dest.display()))?;
        done += chunk.len() as u64;
        if last.elapsed() >= Duration::from_millis(100) {
            last = Instant::now();
            progress(Progress {
                stage: stage.to_string(),
                done,
                total,
            });
        }
    }
    file.flush().await?;
    drop(file);
    progress(Progress {
        stage: stage.to_string(),
        done,
        total,
    });
    Ok(hex::encode(hasher.finalize()))
}

/// Replace `target` with `fresh`. A running exe cannot be overwritten on
/// Windows but it can be renamed, so move the old one aside first.
fn replace_file(fresh: &Path, target: &Path) -> anyhow::Result<()> {
    if target.exists() {
        let old = target.with_extension("exe.old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(target, &old)
            .with_context(|| format!("moving old {} aside", target.display()))?;
        let _ = std::fs::remove_file(&old);
    }
    std::fs::rename(fresh, target).with_context(|| format!("installing {}", target.display()))?;
    Ok(())
}

/// Extract the entries whose file names match `wanted` from a zip, into `dir`
/// with a `.download` suffix. Returns the extracted paths in `wanted` order.
fn extract_from_zip(zip_path: &Path, dir: &Path, wanted: &[&str]) -> anyhow::Result<Vec<PathBuf>> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("download is not a valid zip")?;
    let mut found: Vec<Option<PathBuf>> = vec![None; wanted.len()];
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.replace('\\', "/");
        let base = name.rsplit('/').next().unwrap_or("");
        let Some(idx) = wanted.iter().position(|w| w.eq_ignore_ascii_case(base)) else {
            continue;
        };
        let out = dir.join(staging_name(wanted[idx]));
        let mut writer = std::fs::File::create(&out)?;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut writer, &buf[..n])
                .with_context(|| format!("writing {}", out.display()))?;
        }
        found[idx] = Some(out);
    }
    found
        .into_iter()
        .zip(wanted)
        .map(|(p, w)| p.ok_or_else(|| anyhow!("{w} not found in archive")))
        .collect()
}

/// Download, verify and install a package into `%APPDATA%\YtLocal\bin`.
/// Returns a human-readable summary such as "yt-dlp 2026.08.19".
pub async fn install(
    pkg: Package,
    client: &reqwest::Client,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> anyhow::Result<String> {
    let dir = paths::bin_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;

    let (url, sums_url, sums_name) = match pkg {
        Package::YtDlp => (YTDLP_URL, YTDLP_SUMS, "yt-dlp.exe"),
        Package::Ffmpeg => (FFMPEG_URL, FFMPEG_SUMS, FFMPEG_ZIP),
        Package::Deno => (DENO_URL, DENO_SUMS, DENO_ZIP),
    };

    progress(Progress {
        stage: "Fetching checksum".into(),
        done: 0,
        total: None,
    });
    let sums = fetch_text(client, sums_url).await?;
    let expected = find_sha256(&sums, sums_name)
        .ok_or_else(|| anyhow!("checksum for {sums_name} not found in release"))?;

    let download_path = match pkg {
        Package::YtDlp => dir.join(staging_name(sums_name)),
        _ => dir.join(format!("{sums_name}.download")),
    };
    let result = async {
        let actual = download_file(client, url, &download_path, "Downloading", progress).await?;
        if actual != expected {
            bail!("checksum mismatch (expected {expected}, got {actual}); download was corrupted");
        }

        progress(Progress {
            stage: "Installing".into(),
            done: 0,
            total: None,
        });
        let fresh: Vec<(Tool, PathBuf)> = match pkg {
            Package::YtDlp => vec![(Tool::YtDlp, download_path.clone())],
            Package::Ffmpeg | Package::Deno => {
                let tools = pkg.tools();
                let names: Vec<&str> = tools.iter().map(|t| t.exe_name()).collect();
                let (zip, d) = (download_path.clone(), dir.clone());
                let extracted =
                    tokio::task::spawn_blocking(move || extract_from_zip(&zip, &d, &names))
                        .await??;
                tools.iter().copied().zip(extracted).collect()
            }
        };

        let mut versions = Vec::new();
        for (tool, path) in fresh {
            // Verify before installing so a bad download never replaces a good tool.
            let version = probe(tool, &path)
                .await
                .map_err(|e| anyhow!("downloaded {} does not run: {e}", tool.label()))?;
            replace_file(&path, &dir.join(tool.exe_name()))?;
            versions.push(format!("{} {version}", tool.label()));
        }
        Ok(versions.join(", "))
    }
    .await;

    // Clean up leftovers whatever happened.
    let _ = tokio::fs::remove_file(&download_path).await;
    for tool in pkg.tools() {
        let _ = tokio::fs::remove_file(dir.join(staging_name(tool.exe_name()))).await;
    }
    result
}

/// Update yt-dlp in place with `yt-dlp -U`. If self-update is not possible
/// (pip/winget installs, read-only folder), fall back to a fresh download
/// into our bin folder, which takes priority over PATH.
pub async fn update_ytdlp(
    current: Option<&Path>,
    client: &reqwest::Client,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> anyhow::Result<String> {
    if let Some(path) = current {
        progress(Progress {
            stage: "Running yt-dlp -U".into(),
            done: 0,
            total: None,
        });
        let out = crate::process::run_capture(path, ["-U"], Duration::from_secs(300)).await;
        if let Ok(out) = out
            && out.status.success()
        {
            let version = probe(Tool::YtDlp, path)
                .await
                .map_err(|e| anyhow!("yt-dlp does not run after update: {e}"))?;
            let text = format!("{}{}", out.stdout, out.stderr);
            return Ok(if text.contains("up to date") {
                format!("yt-dlp {version} is up to date")
            } else {
                format!("Updated to yt-dlp {version}")
            });
        }
    }
    install(Package::YtDlp, client, progress)
        .await
        .map(|v| format!("Installed {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a";
    const H2: &str = "7fecfb9e85b39fb98a05c788f78f0b2d1441485da08ba9489550581b41eff385";

    #[test]
    fn sha_from_sums_list() {
        let text = format!("{H2}  ffmpeg-x.zip\n{H1}  yt-dlp.exe\n{H2}  yt-dlp_x86.exe\n");
        assert_eq!(find_sha256(&text, "yt-dlp.exe").as_deref(), Some(H1));
        assert_eq!(find_sha256(&text, "nope.exe"), None);
    }

    #[test]
    fn sha_from_get_filehash_output() {
        let text = format!(
            "Algorithm : SHA256\nHash      : {}\nPath      : C:\\a\\deno\\deno-x86_64-pc-windows-msvc.zip\n",
            H1.to_ascii_uppercase()
        );
        assert_eq!(
            find_sha256(&text, "deno-x86_64-pc-windows-msvc.zip").as_deref(),
            Some(H1)
        );
    }

    #[test]
    fn sha_ambiguous_is_none() {
        let text = format!("{H1}\n{H2}\nsomething.zip\n");
        assert_eq!(find_sha256(&text, "something.zip"), None);
    }
}
