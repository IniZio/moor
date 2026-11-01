//! `moor-notify` against a real `moor controller serve` with a `notify` front socket: the
//! controller takes the front's identity and email, and serves its watches.
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
use support::{write_config, Front};

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
    fn start(root: &Path) -> (Self, PathBuf) {
        let socket = root.join("c.sock");
        let state = root.join("state");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nfronts:\n  notify: {{}}\n",
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

#[test]
fn a_real_controller_enrolls_the_front_and_serves_its_watch() {
    let dir = moor_testkit::short_tempdir("notify-real").unwrap();
    let (_controller, socket) = Controller::start(dir.path());
    let receiver = Receiver::start(&[]);
    let config = write_config(dir.path(), "a@example.com", &receiver.url, None);
    let state = dir.path().join("notify-state");
    std::fs::create_dir(&state).unwrap();
    let front = Front::start(&socket, &config, &state);
    front.wait_for_line("a@example.com: posting idled, setup_failed");
    assert!(
        !front.lines().iter().any(|l| l.contains("controller:")),
        "no controller error: {:#?}",
        front.lines()
    );
}
