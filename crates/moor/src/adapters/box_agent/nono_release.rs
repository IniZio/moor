//! The pinned nono release, downloaded and verified on the host so the Box needs no egress.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use super::sandbox::NONO_VERSION;

/// Where the pinned release assets are published.
const RELEASE_URL: &str = "https://github.com/nolabs-ai/nono/releases/download/v0.80.0";

/// Largest download or unpacked binary accepted.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// One release asset: the Box platform it serves, its file name and sha256.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asset {
    /// `uname -s` of the Box.
    pub(super) os: &'static str,
    /// `uname -m` of the Box.
    pub(super) arch: &'static str,
    /// Release file name.
    pub(super) name: &'static str,
    /// Lowercase hex sha256 of the file.
    pub(super) sha256: &'static str,
}

/// The pinned assets by Box platform.
const ASSETS: &[Asset] = &[
    Asset {
        os: "Linux",
        arch: "x86_64",
        name: "nono-v0.80.0-x86_64-unknown-linux-gnu.tar.gz",
        sha256: "6da665b61999570c10dffc7da78828fe1fb7f3cac53319e505cebecaef17037f",
    },
    Asset {
        os: "Linux",
        arch: "aarch64",
        name: "nono-v0.80.0-aarch64-unknown-linux-gnu.tar.gz",
        sha256: "d2a24c8c37714f85c829b5586ffd81fc2acf336c95372d881ba62140884f22cb",
    },
    Asset {
        os: "Linux",
        arch: "arm64",
        name: "nono-v0.80.0-aarch64-unknown-linux-gnu.tar.gz",
        sha256: "d2a24c8c37714f85c829b5586ffd81fc2acf336c95372d881ba62140884f22cb",
    },
    Asset {
        os: "Darwin",
        arch: "x86_64",
        name: "nono-v0.80.0-x86_64-apple-darwin.tar.gz",
        sha256: "b354e674e6b4643ad5c572d148ae367bb0c5c103410fbc22923c5593a6f7d0b2",
    },
    Asset {
        os: "Darwin",
        arch: "arm64",
        name: "nono-v0.80.0-aarch64-apple-darwin.tar.gz",
        sha256: "ae824167b3d90531b830429ba92b7781379b1c2d79f9b11fdd88b0fd267e4e32",
    },
];

/// Why the nono release could not be produced.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReleaseError {
    /// No pinned asset for this Box platform.
    #[error("nono {NONO_VERSION} has no release for {os} {arch}")]
    Unsupported {
        /// `uname -s` of the Box.
        os: String,
        /// `uname -m` of the Box.
        arch: String,
    },
    /// The download does not match its pinned sha256.
    #[error("the nono {NONO_VERSION} download {asset} failed its sha256 check")]
    Checksum {
        /// Release file name.
        asset: String,
    },
    /// The download failed.
    #[error("could not download nono {NONO_VERSION} on the host: {0}")]
    Download(String),
    /// The archive has no usable `nono` binary.
    #[error("could not unpack nono {NONO_VERSION}: {0}")]
    Extract(String),
}

/// Fetches a URL's body on the host.
pub(crate) trait Fetch: Send + Sync {
    /// The body of `url`.
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Fetches over HTTPS with ureq.
pub(crate) struct UreqFetch;

impl Fetch for UreqFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        let mut response = Agent::new_with_config(config)
            .get(url)
            .call()
            .map_err(|e| e.to_string())?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_vec()
            .map_err(|e| e.to_string())
    }
}

/// The pinned nono release, fetched on the host and cached.
pub(crate) struct NonoRelease {
    /// Downloads assets.
    fetch: Arc<dyn Fetch>,
    /// Directory holding downloaded assets, if caching is on.
    cache: Option<PathBuf>,
    /// Pinned assets by platform.
    assets: &'static [Asset],
}

impl NonoRelease {
    /// A release that downloads with `fetch` and caches under `cache`.
    pub(crate) fn new(fetch: Arc<dyn Fetch>, cache: Option<PathBuf>) -> Self {
        Self::with_assets(fetch, cache, ASSETS)
    }

    /// Like `new` with a different asset table.
    pub(super) fn with_assets(
        fetch: Arc<dyn Fetch>,
        cache: Option<PathBuf>,
        assets: &'static [Asset],
    ) -> Self {
        Self {
            fetch,
            cache,
            assets,
        }
    }

    /// The verified `nono` binary for the Box platform given by `uname -s` and `uname -m`.
    pub(crate) fn binary(&self, os: &str, arch: &str) -> Result<Vec<u8>, ReleaseError> {
        let asset = self
            .assets
            .iter()
            .find(|a| a.os == os && a.arch == arch)
            .ok_or_else(|| ReleaseError::Unsupported {
                os: os.to_owned(),
                arch: arch.to_owned(),
            })?;
        let cached = self
            .cache
            .as_ref()
            .map(|dir| dir.join(format!("nono-v{NONO_VERSION}")).join(asset.name));
        let tarball = match cached.as_deref().and_then(|p| read_verified(p, asset)) {
            Some(bytes) => bytes,
            None => self.download(asset, cached.as_deref())?,
        };
        extract(&tarball)
    }

    /// Fetches and verifies `asset`, then stores it at `cached`.
    fn download(&self, asset: &Asset, cached: Option<&Path>) -> Result<Vec<u8>, ReleaseError> {
        let url = format!("{RELEASE_URL}/{}", asset.name);
        let bytes = self.fetch.get(&url).map_err(ReleaseError::Download)?;
        if !matches_sum(&bytes, asset) {
            return Err(ReleaseError::Checksum {
                asset: asset.name.to_owned(),
            });
        }
        if let Some(path) = cached {
            let _ = store(path, &bytes);
        }
        Ok(bytes)
    }
}

/// The file at `path` if it exists and matches the asset's sha256.
fn read_verified(path: &Path, asset: &Asset) -> Option<Vec<u8>> {
    std::fs::read(path)
        .ok()
        .filter(|bytes| matches_sum(bytes, asset))
}

/// Whether `bytes` hash to the asset's pinned sha256.
fn matches_sum(bytes: &[u8], asset: &Asset) -> bool {
    let digest = Sha256::digest(bytes);
    let hex = digest.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    hex.eq_ignore_ascii_case(asset.sha256)
}

/// Writes `bytes` to `path` through a temp file in the same directory.
fn store(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The `nono` member of a gzip tarball.
fn extract(tarball: &[u8]) -> Result<Vec<u8>, ReleaseError> {
    let fail = |e: std::io::Error| ReleaseError::Extract(e.to_string());
    let mut archive = tar::Archive::new(GzDecoder::new(tarball));
    for entry in archive.entries().map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?;
        if path.file_name().is_some_and(|n| n == "nono") && entry.header().entry_type().is_file() {
            let mut out = Vec::new();
            entry.take(MAX_BYTES).read_to_end(&mut out).map_err(fail)?;
            return Ok(out);
        }
    }
    Err(ReleaseError::Extract(
        "the archive has no nono binary".to_owned(),
    ))
}

#[cfg(test)]
pub(super) mod fake;

#[cfg(test)]
mod tests;
