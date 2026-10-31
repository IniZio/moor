use std::fs;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

use super::{fetch, plain_semver, target};

const T: &str = "x86_64-unknown-linux-musl";

/// Publishes `binary` as release `version` under `dir`, with `sum` as its checksum when given.
fn publish(dir: &Path, version: &str, binary: &[u8], sum: Option<&str>) -> String {
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(binary.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "moor-machine-agent", binary)
        .unwrap();
    let tarball = tar.into_inner().unwrap().finish().unwrap();
    let release = dir.join(format!("v{version}"));
    fs::create_dir_all(&release).unwrap();
    let asset = format!("moor-machine-agent-{T}.tar.gz");
    fs::write(release.join(&asset), &tarball).unwrap();
    let sum = sum.map_or_else(|| format!("{:x}", Sha256::digest(&tarball)), str::to_owned);
    fs::write(
        release.join(format!("{asset}.sha256")),
        format!("{sum}  {asset}\n"),
    )
    .unwrap();
    format!("file://{}", dir.display())
}

#[test]
fn a_matching_checksum_unpacks_the_binary() {
    let (mirror, stage) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let base = publish(mirror.path(), "9.9.9", b"new agent", None);
    let bin = fetch(&base, "9.9.9", T, stage.path()).unwrap();
    assert_eq!(fs::read(bin).unwrap(), b"new agent");
}

#[test]
fn a_wrong_checksum_is_refused_and_nothing_is_unpacked() {
    let (mirror, stage) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let base = publish(mirror.path(), "9.9.9", b"new agent", Some(&"0".repeat(64)));
    let err = fetch(&base, "9.9.9", T, stage.path()).unwrap_err();
    assert!(err.contains("does not match its checksum"), "{err}");
    assert_eq!(fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[test]
fn a_missing_asset_names_what_it_could_not_download() {
    let (mirror, stage) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let base = format!("file://{}", mirror.path().display());
    let err = fetch(&base, "9.9.9", T, stage.path()).unwrap_err();
    assert!(err.contains("cannot download"), "{err}");
    assert!(
        err.contains("v9.9.9/moor-machine-agent-x86_64-unknown-linux-musl.tar.gz"),
        "{err}"
    );
}

#[test]
fn only_published_targets_are_known() {
    assert_eq!(target("linux", "aarch64"), Ok("aarch64-unknown-linux-musl"));
    assert_eq!(target("macos", "x86_64"), Ok("x86_64-apple-darwin"));
    assert!(target("freebsd", "x86_64").unwrap_err().contains("freebsd"));
    assert!(target("linux", "riscv64").is_err());
}

#[test]
fn only_a_plain_semver_names_a_release() {
    assert_eq!(plain_semver("1.2.3").as_deref(), Ok("1.2.3"));
    assert_eq!(
        plain_semver("1.2.3-nightly.4").as_deref(),
        Ok("1.2.3-nightly.4")
    );
    for bad in [
        "v1.2.3",
        "1.2",
        "1.2.3+b",
        "../1.2.3",
        "1.2.3/../x",
        "",
        "1.2.3 ",
    ] {
        assert!(plain_semver(bad).is_err(), "{bad:?}");
    }
}
