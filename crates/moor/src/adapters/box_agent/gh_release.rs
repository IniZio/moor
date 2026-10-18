//! The pinned gh release, downloaded and verified on the host so the Box needs no egress.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use self::gh_assets::GH_ASSETS;

/// The pinned gh release.
pub(super) const GH_VERSION: &str = "2.102.0";

/// Largest download or unpacked binary accepted.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// One release asset: the Box platform it serves, its file name and sha256.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asset {
    /// Release URL the file is published under.
    pub(super) release: &'static str,
    /// `uname -s` of the Box.
    pub(super) os: &'static str,
    /// `uname -m` of the Box.
    pub(super) arch: &'static str,
    /// Release file name.
    pub(super) name: &'static str,
    /// Lowercase hex sha256 of the file.
    pub(super) sha256: &'static str,
}

/// The binary's name in the archive.
const TOOL: &str = "gh";

/// Why a pinned release could not be produced.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReleaseError {
    /// No pinned asset for this Box platform.
    #[error("{TOOL} {GH_VERSION} has no release for {os} {arch}")]
    Unsupported {
        /// `uname -s` of the Box.
        os: String,
        /// `uname -m` of the Box.
        arch: String,
    },
    /// The download does not match its pinned sha256.
    #[error("the {TOOL} {GH_VERSION} download {asset} failed its sha256 check")]
    Checksum {
        /// Release file name.
        asset: String,
    },
    /// The download failed.
    #[error("could not download {0}")]
    Download(String),
    /// The archive has no usable binary.
    #[error("could not unpack {0}")]
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

/// The pinned gh release, fetched on the host and cached.
pub(crate) struct GhRelease {
    /// Downloads assets.
    fetch: Arc<dyn Fetch>,
    /// Directory holding downloaded assets, if caching is on.
    cache: Option<PathBuf>,
    /// Pinned gh assets by platform.
    assets: &'static [Asset],
}

impl GhRelease {
    /// A release that downloads with `fetch` and caches under `cache`.
    pub(crate) fn new(fetch: Arc<dyn Fetch>, cache: Option<PathBuf>) -> Self {
        Self {
            fetch,
            cache,
            assets: GH_ASSETS,
        }
    }

    /// Like `new` with a different asset table.
    #[cfg(test)]
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

    /// The verified `gh` binary for the Box platform given by `uname -s` and `uname -m`.
    pub(super) fn gh_binary(&self, os: &str, arch: &str) -> Result<Vec<u8>, ReleaseError> {
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
            .map(|dir| dir.join(format!("{TOOL}-v{GH_VERSION}")).join(asset.name));
        let archive = match cached.as_deref().and_then(|p| read_verified(p, asset)) {
            Some(bytes) => bytes,
            None => self.download(asset, cached.as_deref())?,
        };
        extract(asset, &archive)
    }

    /// Fetches and verifies `asset`, then stores it at `cached`.
    fn download(&self, asset: &Asset, cached: Option<&Path>) -> Result<Vec<u8>, ReleaseError> {
        let url = format!("{}/{}", asset.release, asset.name);
        let bytes = self
            .fetch
            .get(&url)
            .map_err(|e| ReleaseError::Download(format!("{TOOL} {GH_VERSION} on the host: {e}")))?;
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
    sha256_hex(bytes).eq_ignore_ascii_case(asset.sha256)
}

/// Lowercase hex sha256 of `bytes`.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
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

/// The gh binary in `archive`, a zip or gzip tarball by the asset's file name.
fn extract(asset: &Asset, archive: &[u8]) -> Result<Vec<u8>, ReleaseError> {
    let found = if Path::new(asset.name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        unzip_member(archive, TOOL)
    } else {
        untar_member(archive, TOOL)
    };
    found
        .map_err(|e| ReleaseError::Extract(format!("{TOOL} {GH_VERSION}: {e}")))?
        .ok_or_else(|| {
            ReleaseError::Extract(format!(
                "{TOOL} {GH_VERSION}: the archive has no {TOOL} binary"
            ))
        })
}

/// The first regular file named `member` in a gzip tarball.
fn untar_member(tarball: &[u8], member: &str) -> std::io::Result<Option<Vec<u8>>> {
    let mut archive = tar::Archive::new(GzDecoder::new(tarball));
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        if path.file_name().is_some_and(|n| n == member) && entry.header().entry_type().is_file() {
            let mut out = Vec::new();
            entry.take(MAX_BYTES).read_to_end(&mut out)?;
            return Ok(Some(out));
        }
    }
    Ok(None)
}

/// The first regular file named `member` in a zip.
fn unzip_member(bytes: &[u8], member: &str) -> std::io::Result<Option<Vec<u8>>> {
    let fail = |e: zip::result::ZipError| std::io::Error::other(e);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(fail)?;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(fail)?;
        if file.is_file()
            && file
                .name()
                .is_ok_and(|n| n.rsplit('/').next() == Some(member))
        {
            let mut out = Vec::new();
            file.take(MAX_BYTES).read_to_end(&mut out)?;
            return Ok(Some(out));
        }
    }
    Ok(None)
}

mod gh_assets;

#[cfg(test)]
pub(super) mod fake;

#[cfg(test)]
mod gh_tests;
#[cfg(test)]
mod tests;
