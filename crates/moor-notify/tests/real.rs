//! `moor-notify` against a real `moor controller serve` with a `notify` front socket: the
//! controller answers its claims only when the front's config sets `webhooks: true`.
//!
//! A Box event needs a GitHub clone, which a test cannot make, so nothing here is posted:
//! the tests prove the front claims without refusal and is refused without the setting. The
//! signed POST itself is proved against the scripted controller in `deliver.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::zombie_processes,
    reason = "tests"
)]

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[allow(
    dead_code,
    reason = "this test uses the receiver and the front, not the fake controller"
)]
mod support;

use support::receiver::Receiver;
use support::Front;

/// The `moor` binary next to `moor-notify`, built when it is missing; this crate must not
/// depend on the `moor` crate, so cargo does not build it for these tests.
fn moor_bin() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_moor-notify")).with_file_name("moor");
    if !path.exists() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "moor", "--bin", "moor", "--locked"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "cargo build -p moor failed");
    }
    path
}

/// A controller with no repos and no provider, killed on drop.
struct Controller(Child);

impl Controller {
    /// Starts it with its state under `root` and waits for its `notify` front socket.
    fn start(root: &Path, front_config: &str) -> (Self, PathBuf) {
        let socket = root.join("c.sock");
        let state = root.join("state");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nfronts:\n  notify: {front_config}\n",
                socket.display(),
                state.display()
            ),
        )
        .unwrap();
        let child = Command::new(moor_bin())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MOOR_STATE_DIR", &state)
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let front = state.join("fronts/notify/front");
        for _ in 0..500 {
            if UnixStream::connect(&front).is_ok() {
                return (Self(child), front);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("controller never listened on {}", front.display());
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `moor` with `args` as a local user of the controller at `socket`, and returns the
/// JSON envelope it printed.
fn moor(root: &Path, socket: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(moor_bin())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("MOOR_CONTROLLER", socket)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_real_controller_lets_the_front_claim_a_users_webhooks() {
    let dir = moor_testkit::short_tempdir("notify-real").unwrap();
    let (_controller, front_socket) = Controller::start(dir.path(), "{webhooks: true}");
    let receiver = Receiver::start(&[]);
    let socket = dir.path().join("c.sock");
    assert_eq!(
        moor(dir.path(), &socket, &["user", "enroll"])["class"],
        "ok"
    );
    let added = moor(
        dir.path(),
        &socket,
        &["user", "webhook", "add", &receiver.url],
    );
    assert_eq!(added["class"], "ok", "{added}");
    assert_eq!(added["value"]["key"].as_str().map(str::len), Some(64));

    let mut front = Front::start(&front_socket);
    std::thread::sleep(Duration::from_millis(500));
    assert!(front.is_running(), "{:#?}", front.lines());
    assert!(
        front.lines().iter().all(|l| l.contains("--allow-private")),
        "no controller error: {:#?}",
        front.lines()
    );
    assert!(receiver.posted().is_empty(), "no event, so no POST");
}

#[test]
fn a_front_without_webhooks_true_is_refused_and_exits() {
    let dir = moor_testkit::short_tempdir("notify-real").unwrap();
    let (_controller, front_socket) = Controller::start(dir.path(), "{}");
    let mut front = Front::start(&front_socket);
    let line = front.wait_for_line("webhooks: true");
    assert!(line.starts_with("moor-notify: "), "{line}");
    assert!(!front.wait_for_exit().success());
}
