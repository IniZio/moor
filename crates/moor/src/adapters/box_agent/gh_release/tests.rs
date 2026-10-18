//! Tests for the host-side generic download, cache and checksum.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{Asset, Fetch, GhRelease, ReleaseError};

/// Returns fixed bytes and counts calls.
struct Fake {
    /// Body returned for every URL.
    body: Vec<u8>,
    /// Calls so far.
    calls: AtomicUsize,
    /// URLs asked for.
    urls: std::sync::Mutex<Vec<String>>,
}

impl Fetch for Fake {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.urls.lock().unwrap().push(url.to_owned());
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.body.clone())
    }
}

/// A gzip tarball holding one file at `path`.
fn tarball(path: &str, content: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, path, content).unwrap();
    let raw = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &raw).unwrap();
    gz.finish().unwrap()
}

/// Hex sha256 of `bytes`.
fn sum(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in Sha256::digest(bytes) {
        write!(out, "{b:02x}").unwrap();
    }
    out
}

/// An asset table that serves `bytes` for Linux `x86_64`, `aarch64` and `arm64`.
fn table(bytes: &[u8]) -> &'static [Asset] {
    let sha256: &'static str = Box::leak(sum(bytes).into_boxed_str());
    let row = |arch| Asset {
        release: "https://example.test/release",
        os: "Linux",
        arch,
        name: "gh-test.tar.gz",
        sha256,
    };
    Box::leak(Box::new([row("x86_64"), row("aarch64"), row("arm64")]))
}

/// A fake fetcher and a release over `served` that expects `expected`.
fn setup(
    served: Vec<u8>,
    expected: &[u8],
    cache: Option<std::path::PathBuf>,
) -> (Arc<Fake>, GhRelease) {
    let fake = Arc::new(Fake {
        body: served,
        calls: AtomicUsize::new(0),
        urls: std::sync::Mutex::new(Vec::new()),
    });
    let release = GhRelease::with_assets(fake.clone(), cache, table(expected));
    (fake, release)
}

#[test]
fn downloads_verifies_and_extracts() {
    let tgz = tarball("gh", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn arm64_and_aarch64_both_map() {
    let tgz = tarball("gh", b"BIN");
    let (_, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.gh_binary("Linux", "aarch64").unwrap(), b"BIN");
    assert_eq!(release.gh_binary("Linux", "arm64").unwrap(), b"BIN");
}

#[test]
fn nested_member_is_found() {
    let tgz = tarball("gh-v0/gh", b"BIN");
    let (_, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), b"BIN");
}

#[test]
fn second_call_uses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("gh", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, Some(dir.path().to_owned()));
    release.gh_binary("Linux", "x86_64").unwrap();
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn preseeded_cache_needs_no_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("gh", b"BIN");
    let slot = dir.path().join("gh-v2.102.0");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join("gh-test.tar.gz"), &tgz).unwrap();
    let (fake, release) = setup(Vec::new(), &tgz, Some(dir.path().to_owned()));
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn corrupt_cache_is_refetched_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("gh", b"BIN");
    let slot = dir.path().join("gh-v2.102.0");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join("gh-test.tar.gz"), b"junk").unwrap();
    let (fake, release) = setup(tgz.clone(), &tgz, Some(dir.path().to_owned()));
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(slot.join("gh-test.tar.gz")).unwrap(), tgz);
}

#[test]
fn checksum_mismatch_fails_and_caches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("gh", b"BIN");
    let (_, release) = setup(tarball("gh", b"EVIL"), &tgz, Some(dir.path().to_owned()));
    let err = release.gh_binary("Linux", "x86_64").unwrap_err();
    assert!(matches!(err, ReleaseError::Checksum { .. }), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn unsupported_platform_fetches_nothing() {
    let tgz = tarball("gh", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, None);
    let err = release.gh_binary("Linux", "riscv64").unwrap_err();
    assert!(matches!(err, ReleaseError::Unsupported { .. }), "{err}");
    let message = err.to_string();
    assert!(
        message.contains("Linux") && message.contains("riscv64"),
        "{message}"
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_member_is_an_extract_error() {
    let tgz = tarball("other", b"BIN");
    let (_, release) = setup(tgz.clone(), &tgz, None);
    let err = release.gh_binary("Linux", "x86_64").unwrap_err();
    assert!(matches!(err, ReleaseError::Extract(_)), "{err}");
}

#[test]
fn pinned_table_covers_the_platforms() {
    for (os, arch) in [
        ("Linux", "x86_64"),
        ("Linux", "aarch64"),
        ("Linux", "arm64"),
        ("Darwin", "x86_64"),
        ("Darwin", "arm64"),
    ] {
        assert!(super::GH_ASSETS
            .iter()
            .any(|a| a.os == os && a.arch == arch));
    }
}
