//! The rig for daemon tests: token files, a config, a fake Slack, and the `moor-slack`
//! process, which dials whatever controller the test serves on the rig's socket.

#![allow(
    dead_code,
    clippy::indexing_slicing,
    clippy::large_enum_variant,
    reason = "test code; each suite uses part of the rig"
)]

pub(crate) mod controller;
mod real;
pub(crate) mod slack_fake;

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use moor_core::controller::PROTOCOL;

use controller::FakeController;
use real::RealController;
use slack_fake::{FakeSlack, Mention, APP_TOKEN, BOT_TOKEN};

/// Channel the daemon is configured to answer in.
pub(crate) const CHANNEL: &str = "C0ALLOWED";
/// User the daemon is configured to obey.
pub(crate) const USER: &str = "U0ALLOWED";
/// Ts of the first mention in most tests.
pub(crate) const TS: &str = "1730000000.000100";
/// Box name the first mention maps to.
pub(crate) const BOX_NAME: &str = "slack-1730000000-000100";
/// Pause that lets a wrongly emitted second reply arrive before the test counts replies.
pub(crate) const QUIET: Duration = Duration::from_millis(600);

/// The fake `claude`: it records what it got next to the path it runs as, and answers by
/// the token it read on fd 3.
const FAKE_CLAUDE: &str = include_str!("fake-claude.sh");

/// The fake `claude`, written once for this test binary. A file written while another test
/// thread forks can fail to exec with "Text file busy" until that fork execs, so the first
/// run is retried, and each rig links to this one file.
fn shared_fake_claude() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fake-claude-{}", std::process::id()));
        std::fs::write(&path, FAKE_CLAUDE).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..200 {
            match Command::new(&path).arg("--version").output() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => {
                    other.unwrap();
                    break;
                }
            }
        }
        path
    })
}

/// A private root with token files, a config and a fake Slack; the daemon runs once spawned.
pub(crate) struct Rig {
    /// Private root, removed on drop.
    root: tempfile::TempDir,
    /// The fake Slack the daemon talks to.
    pub(crate) slack: FakeSlack,
    /// The daemon process, once spawned.
    daemon: Option<Child>,
}

/// Writes `text` to `path` readable by its owner only.
fn secret_file(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

impl Rig {
    /// Writes both token files and the config; nothing runs yet.
    pub(crate) fn new() -> Self {
        // Kept short: Unix socket paths are limited to about 104 bytes.
        let root = tempfile::Builder::new().prefix("ms").tempdir().unwrap();
        let slack = FakeSlack::start();
        let rig = Self {
            root,
            slack,
            daemon: None,
        };
        secret_file(&rig.path("app.token"), APP_TOKEN);
        secret_file(&rig.path("bot.token"), BOT_TOKEN);
        rig.write_config(
            &rig.path("app.token").display().to_string(),
            &rig.path("bot.token").display().to_string(),
        );
        rig
    }

    /// Writes the config with these token file paths, exactly as given.
    pub(crate) fn write_config(&self, app_token_file: &str, bot_token_file: &str) {
        std::fs::write(
            self.path("slack.yaml"),
            format!(
                "app_token_file: {app_token_file}\nbot_token_file: {bot_token_file}\nchannels: [{CHANNEL}]\nusers: [{USER}]\napi_base: {}\n",
                self.slack.api_base(),
            ),
        )
        .unwrap();
    }

    /// Names `binary` as `coordinator.binary` in the config.
    pub(crate) fn set_coordinator(&self, binary: &Path) {
        let mut config = std::fs::OpenOptions::new()
            .append(true)
            .open(self.path("slack.yaml"))
            .unwrap();
        writeln!(config, "coordinator:\n  binary: {}", binary.display()).unwrap();
    }

    /// Links the fake `claude` into `claude-bin/`, where it keeps one `run-<n>` dir of
    /// records per run, and names it in the config. It prints `version` for `--version`
    /// when given.
    pub(crate) fn use_fake_claude(&self, version: Option<&str>) -> PathBuf {
        let dir = self.path("claude-bin");
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(shared_fake_claude(), dir.join("claude")).unwrap();
        if let Some(version) = version {
            std::fs::write(dir.join("version"), version).unwrap();
        }
        self.set_coordinator(&dir.join("claude"));
        dir
    }

    /// A scripted controller and the daemon, with the daemon connected to Slack.
    pub(crate) fn start() -> (Self, FakeController) {
        let mut rig = Self::new();
        let controller = rig.serve(PROTOCOL);
        rig.spawn_daemon();
        rig.slack.wait_connected();
        (rig, controller)
    }

    /// `name` inside the rig's root.
    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// The controller socket the daemon dials.
    pub(crate) fn socket(&self) -> PathBuf {
        self.path("c.sock")
    }

    /// Serves a scripted controller that speaks `protocol` on the rig's socket.
    pub(crate) fn serve(&self, protocol: u32) -> FakeController {
        FakeController::serve(&self.socket(), protocol)
    }

    /// Starts a real controller with no repos on the rig's socket.
    pub(crate) fn real_controller(&self) -> RealController {
        RealController::start(self.root.path(), &self.socket())
    }

    /// Spawns the daemon with its state dir in the rig's root.
    pub(crate) fn spawn_daemon(&mut self) {
        let state = self.path("state-slack");
        self.spawn_daemon_with(|daemon| {
            daemon.arg("--state-dir").arg(state);
        });
    }

    /// Spawns the daemon with a clean environment, then the arguments and variables that
    /// `extra` adds; its stderr goes to a file.
    pub(crate) fn spawn_daemon_with(&mut self, extra: impl FnOnce(&mut Command)) {
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_moor-slack"));
        daemon
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path("home"))
            .arg("--socket")
            .arg(self.socket())
            .arg("--config")
            .arg(self.path("slack.yaml"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.path("slack.err")).unwrap());
        extra(&mut daemon);
        self.daemon = Some(daemon.spawn().unwrap());
    }

    /// Kills the daemon and waits for it to exit.
    pub(crate) fn kill_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Polls the daemon's stderr until it contains `text`; fails the test after ten seconds.
    pub(crate) fn wait_stderr(&self, text: &str) {
        for _ in 0..500 {
            if self.stderr().contains(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("stderr never showed {text:?}: {}", self.stderr());
    }

    /// Everything the daemon wrote to stderr so far.
    pub(crate) fn stderr(&self) -> String {
        std::fs::read_to_string(self.path("slack.err")).unwrap_or_default()
    }

    /// The daemon's exit status, waiting up to `limit` for it to exit.
    pub(crate) fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        let child = self.daemon.as_mut()?;
        let step = Duration::from_millis(20);
        let mut waited = Duration::ZERO;
        while waited < limit {
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(step);
            waited += step;
        }
        None
    }

    /// Pushes a mention in the allowed channel from `user`, with event and envelope ids derived from `n`.
    pub(crate) fn mention(
        &self,
        n: u32,
        user: &str,
        text: &str,
        ts: &str,
        thread_ts: Option<&str>,
    ) {
        self.slack.send_mention(&Mention {
            event_id: &format!("Ev{n}"),
            envelope_id: &format!("env-{n}"),
            channel: CHANNEL,
            user,
            text,
            ts,
            thread_ts,
        });
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(child) = self.daemon.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
