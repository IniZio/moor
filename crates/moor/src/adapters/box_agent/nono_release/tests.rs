//! Tests for the host-side nono download, cache and checksum.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::{Asset, Fetch, NonoRelease, ReleaseError};

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
        name: "nono-test.tar.gz",
        sha256,
    };
    Box::leak(Box::new([row("x86_64"), row("aarch64"), row("arm64")]))
}

/// A fake fetcher and a release over `served` that expects `expected`.
fn setup(
    served: Vec<u8>,
    expected: &[u8],
    cache: Option<std::path::PathBuf>,
) -> (Arc<Fake>, NonoRelease) {
    let fake = Arc::new(Fake {
        body: served,
        calls: AtomicUsize::new(0),
        urls: std::sync::Mutex::new(Vec::new()),
    });
    let release = NonoRelease::with_assets(fake.clone(), cache, table(expected));
    (fake, release)
}

#[test]
fn downloads_verifies_and_extracts() {
    let tgz = tarball("nono", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn arm64_and_aarch64_both_map() {
    let tgz = tarball("nono", b"BIN");
    let (_, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.binary("Linux", "aarch64").unwrap(), b"BIN");
    assert_eq!(release.binary("Linux", "arm64").unwrap(), b"BIN");
}

#[test]
fn nested_member_is_found() {
    let tgz = tarball("nono-v0/nono", b"BIN");
    let (_, release) = setup(tgz.clone(), &tgz, None);
    assert_eq!(release.binary("Linux", "x86_64").unwrap(), b"BIN");
}

#[test]
fn second_call_uses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("nono", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, Some(dir.path().to_owned()));
    release.binary("Linux", "x86_64").unwrap();
    assert_eq!(release.binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn preseeded_cache_needs_no_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("nono", b"BIN");
    let slot = dir.path().join("nono-v0.80.0");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join("nono-test.tar.gz"), &tgz).unwrap();
    let (fake, release) = setup(Vec::new(), &tgz, Some(dir.path().to_owned()));
    assert_eq!(release.binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn corrupt_cache_is_refetched_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("nono", b"BIN");
    let slot = dir.path().join("nono-v0.80.0");
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join("nono-test.tar.gz"), b"junk").unwrap();
    let (fake, release) = setup(tgz.clone(), &tgz, Some(dir.path().to_owned()));
    assert_eq!(release.binary("Linux", "x86_64").unwrap(), b"BIN");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(slot.join("nono-test.tar.gz")).unwrap(), tgz);
}

#[test]
fn checksum_mismatch_fails_and_caches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let tgz = tarball("nono", b"BIN");
    let (_, release) = setup(tarball("nono", b"EVIL"), &tgz, Some(dir.path().to_owned()));
    let err = release.binary("Linux", "x86_64").unwrap_err();
    assert!(matches!(err, ReleaseError::Checksum { .. }), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn unsupported_platform_fetches_nothing() {
    let tgz = tarball("nono", b"BIN");
    let (fake, release) = setup(tgz.clone(), &tgz, None);
    let err = release.binary("Linux", "riscv64").unwrap_err();
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
    let err = release.binary("Linux", "x86_64").unwrap_err();
    assert!(matches!(err, ReleaseError::Extract(_)), "{err}");
}

#[test]
fn pinned_table_covers_the_four_platforms() {
    for (os, arch) in [
        ("Linux", "x86_64"),
        ("Linux", "aarch64"),
        ("Linux", "arm64"),
        ("Darwin", "x86_64"),
        ("Darwin", "arm64"),
    ] {
        assert!(super::ASSETS.iter().any(|a| a.os == os && a.arch == arch));
    }
}

/// A release over the pinned table whose every download returns `body`.
fn pinned(body: Vec<u8>) -> (Arc<Fake>, NonoRelease) {
    let fake = Arc::new(Fake {
        body,
        calls: AtomicUsize::new(0),
        urls: std::sync::Mutex::new(Vec::new()),
    });
    (fake.clone(), NonoRelease::new(fake, None))
}

#[test]
fn fork_assets_come_from_the_fork_release() {
    const FORK: &str = "https://github.com/IniZio/nono/releases/download/v0.80.0-moor.1";
    const LINUX_X86_64: &str = "6204caef57aac110d491f01dcceb2745312cc7bdeb1db95576f77e4c92f7c6be";
    const LINUX_AARCH64: &str = "aa9ef2123e52dc5c086b80eceb4fa4918fc451220180b89aabbb8b561e73d198";
    const DARWIN_ARM64: &str = "ff84a3a50bce1781f3f89b62e8760c30ee187d22ec1239374bfb6771b7bf7658";
    for (os, arch, file, sha) in [
        (
            "Linux",
            "x86_64",
            "nono-v0.80.0-x86_64-unknown-linux-gnu.tar.gz",
            LINUX_X86_64,
        ),
        (
            "Linux",
            "aarch64",
            "nono-v0.80.0-aarch64-unknown-linux-gnu.tar.gz",
            LINUX_AARCH64,
        ),
        (
            "Linux",
            "arm64",
            "nono-v0.80.0-aarch64-unknown-linux-gnu.tar.gz",
            LINUX_AARCH64,
        ),
        (
            "Darwin",
            "arm64",
            "nono-v0.80.0-aarch64-apple-darwin.tar.gz",
            DARWIN_ARM64,
        ),
    ] {
        let (fake, release) = pinned(b"junk".to_vec());
        let err = release.binary(os, arch).unwrap_err();
        assert!(
            matches!(err, ReleaseError::Checksum { .. }),
            "{os} {arch}: {err}"
        );
        assert_eq!(
            fake.urls.lock().unwrap().as_slice(),
            [format!("{FORK}/{file}")],
            "{os} {arch}"
        );
        let asset = super::ASSETS
            .iter()
            .find(|a| a.os == os && a.arch == arch)
            .unwrap();
        assert_eq!(asset.sha256, sha, "{os} {arch}");
    }
}

#[test]
fn darwin_x86_64_stays_on_upstream() {
    let (fake, release) = pinned(b"junk".to_vec());
    let err = release.binary("Darwin", "x86_64").unwrap_err();
    assert!(matches!(err, ReleaseError::Checksum { .. }), "{err}");
    let urls = fake.urls.lock().unwrap().clone();
    assert!(
        urls.first().is_some_and(
            |u| u.starts_with("https://github.com/nolabs-ai/nono/releases/download/v0.80.0/")
        ),
        "{urls:?}"
    );
    let asset = super::ASSETS
        .iter()
        .find(|a| a.os == "Darwin" && a.arch == "x86_64")
        .unwrap();
    assert_eq!(
        asset.sha256,
        "b354e674e6b4643ad5c572d148ae367bb0c5c103410fbc22923c5593a6f7d0b2"
    );
}
