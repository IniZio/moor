//! `update` and `run --auto-update`, on a copy of the agent binary and a release mirror under a
//! temp dir, so no test touches the network or the test binary itself.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    clippy::disallowed_methods,
    reason = "tests"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use flate2::write::GzEncoder;
use flate2::Compression;
use moor_client::link::{self, Frame};
use moor_machine_agent::State;
use sha2::{Digest, Sha256};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

const NEW: &[u8] = b"#!/bin/sh\necho swapped\n";

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => "x86_64-apple-darwin",
    }
}

/// A copy of the agent binary in `dir`, which is what the test updates.
fn agent_copy(dir: &Path) -> PathBuf {
    let copy = dir.join("moor-machine-agent");
    fs::copy(env!("CARGO_BIN_EXE_moor-machine-agent"), &copy).unwrap();
    copy
}

/// Publishes `binary` as release `version` under `dir`; `sum` replaces the true checksum.
fn publish(dir: &Path, version: &str, binary: &[u8], sum: Option<&str>) {
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
    let asset = format!("moor-machine-agent-{}.tar.gz", target());
    fs::write(release.join(&asset), &tarball).unwrap();
    let sum = sum.map_or_else(|| format!("{:x}", Sha256::digest(&tarball)), str::to_owned);
    fs::write(
        release.join(format!("{asset}.sha256")),
        format!("{sum}  {asset}\n"),
    )
    .unwrap();
}

fn agent(exe: &Path, mirror: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).env(
        "MOOR_AGENT_RELEASES",
        format!("file://{}", mirror.display()),
    );
    cmd
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_good_checksum_swaps_the_binary_and_names_both_versions() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    publish(&mirror, "9.9.9", NEW, None);
    let out = agent(&exe, &mirror, &["update", "--version", "9.9.9"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("updated from 0.2.0 to 9.9.9"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read(&exe).unwrap(), NEW);
    assert_ne!(fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0);
    let ran = Command::new(&exe).output().unwrap();
    assert_eq!(ran.stdout, b"swapped\n");
}

#[test]
fn a_bad_checksum_is_refused_and_the_old_binary_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let before = fs::read(&exe).unwrap();
    publish(&mirror, "9.9.9", NEW, Some(&"0".repeat(64)));
    let out = agent(&exe, &mirror, &["update", "--version", "9.9.9"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("does not match its checksum"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read(&exe).unwrap(), before);
}

#[test]
fn a_release_that_is_not_published_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    fs::create_dir_all(&mirror).unwrap();
    let before = fs::read(&exe).unwrap();
    let out = agent(&exe, &mirror, &["update", "--version", "9.9.9"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("cannot download"), "{}", stderr(&out));
    assert_eq!(fs::read(&exe).unwrap(), before);
}

#[test]
fn a_version_that_is_not_plain_semver_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let out = agent(&exe, &mirror, &["update", "--version", "../9.9.9"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("plain semantic version"),
        "{}",
        stderr(&out)
    );
}

/// A server that answers every agent's hello with `UpdateRequired { version: 9.9.9 }`.
async fn demanding_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut ws = link::accept(stream).await.unwrap();
                let _ = link::recv(&mut ws, Duration::from_secs(5)).await;
                let ask = Frame::UpdateRequired {
                    version: "9.9.9".to_owned(),
                };
                let _ = link::send(&mut ws, &ask).await;
            });
        }
    });
    server
}

/// A state dir under `dir` for an enrolled agent of `server`; the `XDG_STATE_HOME` to use.
fn enrolled(dir: &Path, server: &str) -> PathBuf {
    let state = dir.join("state");
    let home = state.join("moor-machine-agent");
    fs::create_dir_all(&home).unwrap();
    let record = State {
        server: server.to_owned(),
        machine_id: 7,
        owner: "ada@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 1,
    };
    fs::write(
        home.join("state.json"),
        serde_json::to_string(&record).unwrap(),
    )
    .unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    fs::write(
        home.join("id_ed25519"),
        key.to_openssh(LineEnding::LF).unwrap(),
    )
    .unwrap();
    state
}

/// Runs `cmd` off the async runtime, which the demanding server needs to keep turning.
async fn finish(mut cmd: Command) -> Output {
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_update_installs_the_release_and_restarts_with_the_same_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let marker = dir.path().join("marker");
    let script = format!(
        "#!/bin/sh\necho \"$MOOR_AGENT_UPDATED_TO $*\" > {}\n",
        marker.display()
    );
    publish(&mirror, "9.9.9", script.as_bytes(), None);
    let state = enrolled(dir.path(), &demanding_server().await);
    let mut cmd = agent(&exe, &mirror, &["run", "--auto-update"]);
    cmd.env("XDG_STATE_HOME", state);
    let out = finish(cmd).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        "9.9.9 run --auto-update\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_without_auto_update_exits_naming_the_update_command() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let before = fs::read(&exe).unwrap();
    publish(&mirror, "9.9.9", NEW, None);
    let state = enrolled(dir.path(), &demanding_server().await);
    let mut cmd = agent(&exe, &mirror, &["run"]);
    cmd.env("XDG_STATE_HOME", state);
    let out = finish(cmd).await;
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("moor-machine-agent update --version 9.9.9"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read(&exe).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_auto_update_says_why_and_exits_non_zero() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    fs::create_dir_all(&mirror).unwrap();
    let before = fs::read(&exe).unwrap();
    let state = enrolled(dir.path(), &demanding_server().await);
    let mut cmd = agent(&exe, &mirror, &["run", "--auto-update"]);
    cmd.env("XDG_STATE_HOME", state);
    let out = finish(cmd).await;
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("the update failed"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read(&exe).unwrap(), before);
}
