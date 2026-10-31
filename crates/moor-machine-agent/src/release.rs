//! Replaces the running agent with a release published on the project's GitHub releases.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use moor_client::oidc::Http;
use sha2::{Digest, Sha256};

/// Where releases are published; `MOOR_AGENT_RELEASES` replaces it.
const RELEASES: &str = "https://github.com/IniZio/moor/releases/download";
/// The binary inside every tarball, and the first part of every asset's name.
const BINARY: &str = "moor-machine-agent";
/// The most a download or an unpacked binary may take.
const LIMIT: u64 = 256 << 20;
/// How long one download may take.
const DOWNLOAD: Duration = Duration::from_secs(300);

/// What a finished update changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updated {
    /// The release that was running.
    pub from: String,
    /// The release that is installed now.
    pub to: String,
    /// The executable that was replaced, found before the swap while its path was still valid.
    pub exe: PathBuf,
}

/// `version` as written in a release tag, when it is a plain semantic version.
///
/// # Errors
/// It is not `MAJOR.MINOR.PATCH`, optionally with a pre-release such as `-nightly.4`.
pub(crate) fn plain_semver(version: &str) -> Result<String, String> {
    match semver::Version::parse(version) {
        Ok(v) if v.build.is_empty() && v.to_string() == version => Ok(v.to_string()),
        _ => Err(format!("{version:?} is not a plain semantic version")),
    }
}

/// The release target of a host running `os` on `arch`.
fn target(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        _ => Err(format!("no release is published for {os} on {arch}")),
    }
}

/// Replaces the running executable with release `version` of this agent.
///
/// # Errors
/// The version is not plain semver, this host has no published target, the download fails
/// or does not match its checksum, or the executable cannot be replaced.
pub fn update(version: &str) -> Result<Updated, String> {
    let to = plain_semver(version)?;
    let target = target(std::env::consts::OS, std::env::consts::ARCH)?;
    let base = std::env::var("MOOR_AGENT_RELEASES").unwrap_or_else(|_| RELEASES.to_owned());
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this executable: {e}"))?;
    let stage = tempfile::tempdir().map_err(|e| format!("cannot make a staging dir: {e}"))?;
    let new = fetch(&base, &to, target, stage.path())?;
    self_replace::self_replace(&new)
        .map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
    Ok(Updated {
        from: crate::VERSION.to_owned(),
        to,
        exe,
    })
}

/// Downloads release `version`'s tarball for `target` from under `base`, checks it against its
/// published checksum, and unpacks the binary into `stage`.
fn fetch(base: &str, version: &str, target: &str, stage: &Path) -> Result<PathBuf, String> {
    let url = format!(
        "{}/v{version}/{BINARY}-{target}.tar.gz",
        base.trim_end_matches('/')
    );
    let tarball = get(&url)?;
    let sum = get(&format!("{url}.sha256"))?;
    let want = String::from_utf8_lossy(&sum)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let got = format!("{:x}", Sha256::digest(&tarball));
    if got != want {
        return Err(format!(
            "{url} does not match its checksum (published {want:?}, downloaded {got})"
        ));
    }
    unpack(&tarball, stage).map_err(|e| format!("cannot unpack {url}: {e}"))
}

/// The bytes at `url`, which is `https` or, for a mirror on this machine, `file://`.
fn get(url: &str) -> Result<Vec<u8>, String> {
    let got = url.strip_prefix("file://").map_or_else(
        || Http::downloads(DOWNLOAD).get_bytes(url, LIMIT),
        |path| fs::read(path).map_err(|e| e.to_string()),
    );
    got.map_err(|e| {
        format!("cannot download {url}: {e}; a release just made may still be building")
    })
}

/// Writes the agent binary of the gzipped `tarball` into `stage`.
fn unpack(tarball: &[u8], stage: &Path) -> Result<PathBuf, String> {
    let mut files = tar::Archive::new(GzDecoder::new(tarball));
    for entry in files.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let is_binary = entry.header().entry_type().is_file()
            && entry.path().map_err(|e| e.to_string())?.file_name() == Some(BINARY.as_ref());
        if is_binary {
            let mut bytes = Vec::new();
            entry
                .take(LIMIT)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let out = stage.join(BINARY);
            fs::write(&out, bytes).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err(format!("it holds no {BINARY}"))
}

#[cfg(test)]
mod tests;
