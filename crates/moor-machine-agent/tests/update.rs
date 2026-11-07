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
use moor_client::link::{self, ops, Channel, Frame};
use moor_machine_agent::State;
use moor_testkit::ChildGuard;
use serde_json::json;
use sha2::{Digest, Sha256};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tokio::sync::{mpsc, oneshot};

const NEW: &[u8] = b"#!/bin/sh\necho swapped\n";

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => "x86_64-apple-darwin",
    }
}

/// A copy of the agent binary in `dir`, which is what the test updates. `cp` writes it, since a
/// file this process held open for writing would be busy in any child another test forks.
fn agent_copy(dir: &Path) -> PathBuf {
    let copy = dir.join("moor-machine-agent");
    let mut cp = Command::new("cp");
    cp.arg(env!("CARGO_BIN_EXE_moor-machine-agent")).arg(&copy);
    assert!(ChildGuard::new(cp.spawn().unwrap())
        .wait()
        .unwrap()
        .success());
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

/// A server that links every agent and welcomes its nth link as release `versions[n]`, or the
/// last; each link, as the way to open streams to its agent.
async fn welcoming_server(
    versions: &'static [&'static str],
) -> (String, mpsc::UnboundedReceiver<Opener>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let (links, linked) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let last = std::iter::repeat(versions.last().unwrap());
        for &version in versions.iter().chain(last) {
            let (tcp, _) = listener.accept().await.unwrap();
            let links = links.clone();
            tokio::spawn(async move {
                let Ok(mut ws) = link::accept(tcp).await else {
                    return;
                };
                let answer = Duration::from_secs(5);
                let nonce = Frame::Challenge {
                    nonce: "n".to_owned(),
                };
                let welcome = Frame::Welcome {
                    version: version.to_owned(),
                };
                let shook = link::recv(&mut ws, answer).await.is_ok()
                    && link::send(&mut ws, &nonce).await.is_ok()
                    && link::recv(&mut ws, answer).await.is_ok()
                    && link::send(&mut ws, &welcome).await.is_ok();
                if !shook {
                    return;
                }
                let carried = link::carry(ws, link::KEEPALIVE);
                let (ask, asks) = mpsc::channel(4);
                let _ = links.send(Opener(ask, carried.tell));
                let mode = link::Mode::Server;
                link::drive(carried.socket, mode, asks, drop, std::future::pending()).await;
            });
        }
    });
    (server, linked)
}

/// Opens streams to one linked agent, and sends it frames.
struct Opener(mpsc::Sender<link::Asked>, mpsc::Sender<Frame>);

impl Opener {
    /// A new stream carrying `op`.
    async fn send(&self, op: serde_json::Value) -> Channel {
        let (opened, ch) = oneshot::channel();
        self.0.send(opened).await.unwrap();
        let mut ch = ch.await.unwrap().unwrap();
        ops::write_line(&mut ch, &op).await.unwrap();
        ch
    }
}

/// A crabbox whose warmup claims its slug and waits for `$HOME/go`.
const SLOW_WARMUP: &str = r#"#!/bin/sh
case "$1" in
  warmup) touch "$HOME/claimed"; while [ ! -e "$HOME/go" ]; do sleep 0.02; done ;;
  list) echo '[]' ;;
esac
"#;

/// A script release that writes what it was started as to `marker`.
fn marking(marker: &Path) -> String {
    format!(
        "#!/bin/sh\necho \"$MOOR_AGENT_UPDATED_TO $*\" > {}\n",
        marker.display()
    )
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let by = std::time::Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(std::time::Instant::now() < by, "timed out waiting: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn logged(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Starts `cmd` with its stderr in `log`.
fn spawn(mut cmd: Command, log: &Path) -> ChildGuard {
    cmd.stderr(fs::File::create(log).unwrap());
    ChildGuard::new(cmd.spawn().unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_agent_updates_in_the_background_and_restarts_once_its_boot_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let (home, marker) = (dir.path().join("home"), dir.path().join("marker"));
    fs::create_dir_all(&home).unwrap();
    let crabbox = home.join("crabbox");
    moor_testkit::write_executable(&crabbox, SLOW_WARMUP).unwrap();
    let release = marking(&marker);
    publish(&mirror, "9.9.9", release.as_bytes(), None);
    let (server, mut links) = welcoming_server(&[env!("CARGO_PKG_VERSION"), "9.9.9"]).await;
    let crabbox = crabbox.to_str().unwrap();
    let args = [
        "run",
        "--runtime",
        "fake",
        "--crabbox",
        crabbox,
        "--auto-update",
    ];
    let mut cmd = agent(&exe, &mirror, &args);
    cmd.env("XDG_STATE_HOME", enrolled(dir.path(), &server))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"));
    let log = dir.path().join("agent.log");
    let _agent = spawn(cmd, &log);
    let old = links.recv().await.unwrap();

    let warmup = json!({"op": "lease", "verb": "warmup", "slug": "moor-1a2b3c4d-7"});
    let ch = old.send(warmup).await;
    until("crabbox claims the slug", || home.join("claimed").exists()).await;
    drop(ch);
    old.1.send(Frame::Relink {}).await.unwrap();
    let opener = links.recv().await.unwrap();
    until("the release is swapped in", || {
        fs::read(&exe).unwrap() == release.as_bytes()
    })
    .await;
    let mut status = opener.send(json!({"op": "status"})).await;
    let reply: ops::Reply = ops::read_line(&mut status).await.unwrap();
    assert!(matches!(reply, ops::Reply::Status(_)), "{reply:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!marker.exists(), "restarted mid-boot: {}", logged(&log));

    fs::write(home.join("go"), "").unwrap();
    until("the agent restarts", || marker.exists()).await;
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        format!("9.9.9 {}\n", args.join(" "))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_linked_to_a_stable_server_updates_to_it_and_never_to_a_nightly() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let marker = dir.path().join("marker");
    publish(&mirror, "0.3.0", marking(&marker).as_bytes(), None);
    publish(&mirror, "0.4.0-nightly.1", NEW, None);
    let (server, _links) = welcoming_server(&["0.3.0"]).await;
    let mut cmd = agent(&exe, &mirror, &["run", "--auto-update"]);
    cmd.env("XDG_STATE_HOME", enrolled(dir.path(), &server));
    let log = dir.path().join("agent.log");
    let _agent = spawn(cmd, &log);
    until("the agent restarts", || marker.exists()).await;
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "0.3.0 run --auto-update\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_auto_update_a_linked_agent_says_an_update_exists_and_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    let before = fs::read(&exe).unwrap();
    publish(&mirror, "9.9.9", NEW, None);
    let (server, mut links) = welcoming_server(&["9.9.9"]).await;
    let mut cmd = agent(&exe, &mirror, &["run"]);
    cmd.env("XDG_STATE_HOME", enrolled(dir.path(), &server));
    let log = dir.path().join("agent.log");
    let mut agent = spawn(cmd, &log);
    let opener = links.recv().await.unwrap();
    let said = "the server runs 9.9.9 and this agent 0.2.0; run `moor-machine-agent update --version 9.9.9`";
    until("the agent says an update exists", || {
        logged(&log).contains(said)
    })
    .await;
    let mut status = opener.send(json!({"op": "status"})).await;
    let reply: ops::Reply = ops::read_line(&mut status).await.unwrap();
    assert!(matches!(reply, ops::Reply::Status(_)), "{reply:?}");
    assert!(agent.try_wait().unwrap().is_none(), "{}", logged(&log));
    assert_eq!(fs::read(&exe).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_background_update_leaves_the_agent_linked_on_its_release() {
    let dir = tempfile::tempdir().unwrap();
    let (mirror, exe) = (dir.path().join("mirror"), agent_copy(dir.path()));
    fs::create_dir_all(&mirror).unwrap();
    let before = fs::read(&exe).unwrap();
    let (server, mut links) = welcoming_server(&["9.9.9"]).await;
    let mut cmd = agent(&exe, &mirror, &["run", "--auto-update"]);
    cmd.env("XDG_STATE_HOME", enrolled(dir.path(), &server));
    let log = dir.path().join("agent.log");
    let mut agent = spawn(cmd, &log);
    let opener = links.recv().await.unwrap();
    until("the update fails", || {
        logged(&log).contains("this agent keeps running 0.2.0")
    })
    .await;
    assert!(
        logged(&log).contains("the update failed"),
        "{}",
        logged(&log)
    );
    let mut status = opener.send(json!({"op": "status"})).await;
    let reply: ops::Reply = ops::read_line(&mut status).await.unwrap();
    assert!(matches!(reply, ops::Reply::Status(_)), "{reply:?}");
    assert!(agent.try_wait().unwrap().is_none(), "{}", logged(&log));
    assert_eq!(fs::read(&exe).unwrap(), before);
}
