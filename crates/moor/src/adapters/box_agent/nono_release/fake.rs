//! A scripted release download for box agent tests.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use super::{sha256_hex, Asset, Fetch, NonoRelease};

/// Answers every download with one scripted body and records the URLs asked for.
pub(crate) struct FakeFetch {
    /// What each download returns.
    body: Result<Vec<u8>, String>,
    /// The URLs asked for, in order.
    urls: Mutex<Vec<String>>,
}

impl FakeFetch {
    /// A download that returns `body`.
    pub(crate) fn new(body: Result<Vec<u8>, String>) -> Self {
        Self {
            body,
            urls: Mutex::new(Vec::new()),
        }
    }

    /// The URLs asked for so far.
    pub(crate) fn urls(&self) -> Vec<String> {
        self.urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Fetch for FakeFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.to_owned());
        self.body.clone()
    }
}

/// A release whose downloads all fail, for tests that never install the tool.
pub(crate) fn offline_release() -> Arc<NonoRelease> {
    let fetch = Arc::new(FakeFetch::new(Err("offline".to_owned())));
    Arc::new(NonoRelease::new(fetch, None))
}

/// A gzip tarball holding one `nono` file.
pub(crate) fn tarball(content: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(content.len()).unwrap());
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, "nono", content).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

/// A release whose only asset, for Linux `x86_64`, is `tarball`.
pub(crate) fn release_of(fetch: &Arc<FakeFetch>, tarball: &[u8]) -> NonoRelease {
    let sha256: &'static str = Box::leak(sha256_hex(tarball).into_boxed_str());
    let asset = Asset {
        os: "Linux",
        arch: "x86_64",
        name: "nono-test.tar.gz",
        sha256,
    };
    let assets: &'static [Asset] = Box::leak(Box::new([asset]));
    let fetch: Arc<dyn Fetch> = fetch.clone();
    NonoRelease::with_assets(fetch, None, assets)
}

/// The `nono` binary that `ready_release` serves.
pub(crate) const READY_BINARY: &[u8] = b"the binary";

/// A release that serves `READY_BINARY` for Linux `x86_64`.
pub(crate) fn ready_release() -> Arc<NonoRelease> {
    let tarball = tarball(READY_BINARY);
    let fetch = Arc::new(FakeFetch::new(Ok(tarball.clone())));
    Arc::new(release_of(&fetch, &tarball))
}
