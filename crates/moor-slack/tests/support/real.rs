//! A real `moor controller serve` with no repos and no provider, for the smoke tests.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// Path of the `moor` binary, resolved once per test binary.
static MOOR: OnceLock<PathBuf> = OnceLock::new();

/// The `moor` binary next to `moor-slack`, built once when it is missing.
///
/// `moor-slack` must not depend on the `moor` crate, so cargo does not build
/// the binary for these tests.
fn moor_bin() -> &'static Path {
    MOOR.get_or_init(|| {
        let slack = Path::new(env!("CARGO_BIN_EXE_moor-slack"));
        let path = slack.with_file_name("moor");
        if path.exists() {
            return path;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "moor", "--bin", "moor", "--locked"])
            .current_dir(root)
            .status()
            .expect("run cargo build for moor");
        assert!(status.success(), "cargo build -p moor failed");
        assert!(path.exists(), "moor binary not found at {}", path.display());
        path
    })
}

/// A running controller, killed on drop.
pub(crate) struct RealController(Child);

impl RealController {
    /// Starts the controller on `socket` with its state under `root`, and waits until it listens.
    pub(crate) fn start(root: &Path, socket: &Path) -> Self {
        let config = root.join("c.yaml");
        let state = root.join("state");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
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
        let controller = Self(child);
        for _ in 0..500 {
            if UnixStream::connect(socket).is_ok() {
                return controller;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("controller never listened on {}", socket.display());
    }
}

impl Drop for RealController {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
