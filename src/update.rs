//! Self-update from GitHub Releases.
//!
//! A release is a tag like `v0.2.0` with an asset named `SpeedMeter-Setup-0.2.0.exe`
//! (built by .github/workflows/release.yml). The app reads the latest release from
//! the public API, downloads the installer, checks it against the SHA-256 digest
//! GitHub publishes for every asset, and runs it silently. The installer closes
//! SpeedMeter, upgrades it in place and starts it again.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "PercyGFX/speedmeter";
const ASSET_PREFIX: &str = "SpeedMeter-Setup-";
const USER_AGENT: &str = concat!("SpeedMeter/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex SHA-256 from GitHub, when available.
    pub sha256: Option<String>,
    /// The release page, for "what's new".
    pub page: String,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    Some((it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?, it.next().unwrap_or(Ok(0)).ok()?))
}

pub fn is_newer(candidate: &str, than: &str) -> bool {
    match (parse(candidate), parse(than)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Where the installer puts the app. Automatic installs only happen from here, so a
/// development build or a copy run from Downloads never replaces itself.
pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("SpeedMeter"))
}

pub fn running_installed_copy() -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();
    let exe = crate::sys::exe_path();
    match (install_dir(), exe.parent()) {
        (Some(dir), Some(parent)) => norm(parent) == norm(&dir),
        _ => false,
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build()
}

/// Returns the latest release if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = match agent().get(&url).set("User-Agent", USER_AGENT).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        // No releases published yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(e) => return Err(format!("Couldn't reach GitHub ({e})")),
    };
    let json: serde_json::Value = resp.into_json().map_err(|e| format!("Unexpected reply from GitHub ({e})"))?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    if !is_newer(tag, current()) {
        return Ok(None);
    }
    let asset = json["assets"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.starts_with(ASSET_PREFIX) && n.ends_with(".exe")))
        })
        .ok_or_else(|| format!("Release {tag} has no installer attached"))?;
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_ascii_lowercase()),
        page: json["html_url"].as_str().unwrap_or_default().to_string(),
    }))
}

/// Downloads the installer to %TEMP%, reporting (bytes so far, total) as it goes.
pub fn download(rel: &Release, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    if !rel.url.starts_with("https://") {
        return Err("Refusing a non-HTTPS download".into());
    }
    let resp = agent().get(&rel.url).set("User-Agent", USER_AGENT).call().map_err(|e| format!("Download failed ({e})"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(rel.size);
    let path = std::env::temp_dir().join(format!("SpeedMeter-Setup-{}.exe", rel.version));
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download interrupted ({e})"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        progress(got, total);
    }
    drop(file);
    if let Some(expected) = &rel.sha256 {
        let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if &actual != expected {
            let _ = std::fs::remove_file(&path);
            return Err("The download didn't match GitHub's checksum, so it was discarded".into());
        }
    }
    Ok(path)
}

/// Starts the installer silently. `launch` is what the installer runs afterwards:
/// "--background" for a quiet update, "--open" to come back with the window open.
pub fn install(installer: &Path, launch: &str) -> Result<(), String> {
    std::process::Command::new(installer)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", &format!("/launch={launch}")])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't start the installer ({e})"))
}

/// Background auto-update for the monitor: checks a couple of minutes after start
/// and then twice a day. Only the installed copy updates itself.
pub fn spawn_auto_updater(quit: impl Fn() + Send + 'static) {
    if cfg!(debug_assertions) || !running_installed_copy() {
        return;
    }
    let _ = std::thread::Builder::new().name("auto-update".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(120));
        loop {
            if crate::config::load().auto_update {
                match check() {
                    Ok(Some(rel)) => match download(&rel, |_, _| {}) {
                        Ok(path) => {
                            crate::sys::log(&format!("updating to {}", rel.version));
                            if install(&path, "--background").is_ok() {
                                quit();
                                return;
                            }
                        }
                        Err(e) => crate::sys::log(&format!("update download failed: {e}")),
                    },
                    Ok(None) => {}
                    Err(e) => crate::sys::log(&format!("update check failed: {e}")),
                }
            }
            std::thread::sleep(Duration::from_secs(12 * 3600));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
