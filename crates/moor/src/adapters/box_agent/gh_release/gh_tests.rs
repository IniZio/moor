//! Tests for the host-side gh download, platform choice and unpacking.

use std::io::Write as _;
use std::sync::Arc;

use super::fake::{ready_release_with_gh, FakeFetch, READY_GH};
use super::{sha256_hex, Asset, GhRelease, ReleaseError, GH_VERSION};

const GH_RELEASE: &str = "https://github.com/cli/cli/releases/download/v2.102.0";

/// A release over the pinned gh table whose every download returns junk.
fn pinned() -> (Arc<FakeFetch>, GhRelease) {
    let fetch = Arc::new(FakeFetch::new(Ok(b"junk".to_vec())));
    (fetch.clone(), GhRelease::new(fetch, None))
}

#[test]
fn each_platform_requests_its_own_asset() {
    for (os, arch, file) in [
        ("Linux", "x86_64", "gh_2.102.0_linux_amd64.tar.gz"),
        ("Linux", "aarch64", "gh_2.102.0_linux_arm64.tar.gz"),
        ("Linux", "arm64", "gh_2.102.0_linux_arm64.tar.gz"),
        ("Darwin", "x86_64", "gh_2.102.0_macOS_amd64.zip"),
        ("Darwin", "arm64", "gh_2.102.0_macOS_arm64.zip"),
    ] {
        let (fetch, release) = pinned();
        let err = release.gh_binary(os, arch).unwrap_err();
        assert!(
            matches!(err, ReleaseError::Checksum { .. }),
            "{os} {arch}: {err}"
        );
        assert_eq!(
            fetch.urls(),
            [format!("{GH_RELEASE}/{file}")],
            "{os} {arch}"
        );
    }
}

#[test]
fn unknown_platform_is_unsupported_and_fetches_nothing() {
    for (os, arch) in [
        ("Linux", "riscv64"),
        ("FreeBSD", "x86_64"),
        ("Darwin", "aarch64"),
    ] {
        let (fetch, release) = pinned();
        let err = release.gh_binary(os, arch).unwrap_err();
        assert!(
            matches!(err, ReleaseError::Unsupported { .. }),
            "{os} {arch}: {err}"
        );
        assert!(fetch.urls().is_empty());
    }
}

#[test]
fn gh_table_holds_exactly_the_pinned_sums() {
    let mut sums: Vec<&str> = super::GH_ASSETS.iter().map(|a| a.sha256).collect();
    sums.sort_unstable();
    sums.dedup();
    assert_eq!(
        sums,
        [
            "7862c86c72f43df3a2d93ddde6f473285b4e2af61b494849846827e513ef6484",
            "b245f24eb2bf5f75b426b4c26da3651a107f8d5b6f4fddfbfccc5679041378b3",
            "bb766f710eef8ede859c18578c72c327597cd4c8a85b06001b1f3843c6019386",
            "da922c20d1792e5b2cbf375593d7a658acf034c12c84e007e71c76ef959c337e",
        ]
    );
}

#[test]
fn errors_name_the_tool_and_version() {
    let (_, release) = pinned();
    let message = release
        .gh_binary("Linux", "x86_64")
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("gh") && message.contains(GH_VERSION),
        "{message}"
    );
    let message = release
        .gh_binary("Linux", "riscv64")
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("gh") && message.contains(GH_VERSION),
        "{message}"
    );
}

#[test]
fn ready_release_serves_gh() {
    let release = ready_release_with_gh();
    assert_eq!(release.gh_binary("Linux", "x86_64").unwrap(), READY_GH);
}

/// A zip holding `gh_x/bin/gh` and a decoy.
fn zip_with_gh(content: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("gh_x/LICENSE", opts).unwrap();
    zip.write_all(b"license").unwrap();
    zip.start_file("gh_x/bin/gh", opts).unwrap();
    zip.write_all(content).unwrap();
    zip.finish().unwrap().into_inner()
}

/// A release whose only gh asset, for Darwin `arm64`, is the zip `bytes`.
fn zip_release(
    served: Vec<u8>,
    bytes: &[u8],
    cache: Option<std::path::PathBuf>,
) -> (Arc<FakeFetch>, GhRelease) {
    let sha256: &'static str = Box::leak(sha256_hex(bytes).into_boxed_str());
    let asset = Asset {
        release: "https://example.test/release",
        os: "Darwin",
        arch: "arm64",
        name: "gh-test.zip",
        sha256,
    };
    let assets: &'static [Asset] = Box::leak(Box::new([asset]));
    let fetch = Arc::new(FakeFetch::new(Ok(served)));
    let release = GhRelease::with_assets(fetch.clone(), cache, assets);
    (fetch, release)
}

#[test]
fn zip_asset_yields_the_gh_member() {
    let zip = zip_with_gh(b"GH");
    let (_, release) = zip_release(zip.clone(), &zip, None);
    assert_eq!(release.gh_binary("Darwin", "arm64").unwrap(), b"GH");
}

#[test]
fn zip_without_gh_is_an_extract_error() {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("other", zip::write::SimpleFileOptions::default())
        .unwrap();
    let zip = zip.finish().unwrap().into_inner();
    let (_, release) = zip_release(zip.clone(), &zip, None);
    let err = release.gh_binary("Darwin", "arm64").unwrap_err();
    assert!(matches!(err, ReleaseError::Extract(_)), "{err}");
}

#[test]
fn checksum_mismatch_is_rejected_and_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    let zip = zip_with_gh(b"GH");
    let (_, release) = zip_release(zip_with_gh(b"EVIL"), &zip, Some(dir.path().to_owned()));
    let err = release.gh_binary("Darwin", "arm64").unwrap_err();
    assert!(matches!(err, ReleaseError::Checksum { .. }), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn second_call_hits_the_per_version_cache() {
    let dir = tempfile::tempdir().unwrap();
    let zip = zip_with_gh(b"GH");
    let (fetch, release) = zip_release(zip.clone(), &zip, Some(dir.path().to_owned()));
    release.gh_binary("Darwin", "arm64").unwrap();
    assert_eq!(release.gh_binary("Darwin", "arm64").unwrap(), b"GH");
    assert_eq!(fetch.urls().len(), 1);
    assert!(dir
        .path()
        .join(format!("gh-v{GH_VERSION}"))
        .join("gh-test.zip")
        .is_file());
}
