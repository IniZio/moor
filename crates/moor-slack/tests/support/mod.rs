//! The rig for daemon tests: token files, a config, a fake Slack, and the `moor-slack`
//! process, which dials whatever controller the test serves on the rig's socket.

#![allow(
    dead_code,
    clippy::indexing_slicing,
    clippy::large_enum_variant,
    reason = "test code; each suite uses part of the rig"
)]

pub(crate) mod controller;
pub(crate) mod github_fake;
mod real;
pub(crate) mod slack_fake;

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use moor_core::controller::PROTOCOL;

use controller::FakeController;
use github_fake::FakeGitHub;
pub(crate) use real::{front_socket, RealController};
use slack_fake::{FakeSlack, Mention, APP_TOKEN, BOT_TOKEN};

/// Channel the daemon is configured to answer in.
pub(crate) const CHANNEL: &str = "C0INVITED";
/// User most tests speak as; the daemon obeys any user in a channel it is invited to.
pub(crate) const USER: &str = "U0ALLOWED";
/// Ts of the first mention in most tests.
pub(crate) const TS: &str = "1730000000.000100";
/// Name of the first Box `start_box` starts in thread [`TS`].
pub(crate) const BOX_NAME: &str = "slack-1730000000-000100-1";
/// A token the fake `claude` answers with the tool calls the test wrote.
pub(crate) const MCP: &str = "sk-ant-oat01-mcp-5ea7";
/// The repo the rigs mark as the channel's default.
pub(crate) const REPO: &str = "acme/demo";
/// Pause that lets a wrongly emitted second reply arrive before the test counts replies.
pub(crate) const QUIET: Duration = Duration::from_millis(600);

/// The line the front posts once the scripted controller started Box `name` on its branch.
pub(crate) fn started_line(name: &str) -> String {
    format!("Started Box `{name}` on branch `moor/{name}`.")
}

/// The fake `claude`: it records what it got next to the path it runs as, and answers by
/// the token it read on fd 3.
const FAKE_CLAUDE: &str = include_str!("fake-claude.sh");

/// A private root with token files, a config and a fake Slack; the daemon runs once spawned.
pub(crate) struct Rig {
    /// Private root, removed on drop.
    root: tempfile::TempDir,
    /// The fake Slack the daemon talks to.
    pub(crate) slack: FakeSlack,
    /// The fake GitHub API the daemon checks pasted tokens with.
    pub(crate) github: FakeGitHub,
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
            github: FakeGitHub::start(),
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
                "app_token_file: {app_token_file}\nbot_token_file: {bot_token_file}\napi_base: {}\ngithub_api_base: {}\n",
                self.slack.api_base(),
                self.github.api_base(),
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

    /// Sets `coordinator.idle_minutes`; call it after the coordinator section is written.
    pub(crate) fn set_idle_minutes(&self, minutes: u32) {
        let mut config = std::fs::OpenOptions::new()
            .append(true)
            .open(self.path("slack.yaml"))
            .unwrap();
        writeln!(config, "  idle_minutes: {minutes}").unwrap();
    }

    /// Links the fake `claude` into `claude-bin/`, where it keeps one `run-<n>` dir of
    /// records per run, and names it in the config. It prints `version` for `--version`
    /// when given.
    pub(crate) fn use_fake_claude(&self, version: Option<&str>) -> PathBuf {
        let dir = self.link_fake_claude();
        if let Some(version) = version {
            std::fs::write(dir.join("version"), version).unwrap();
        }
        self.set_coordinator(&dir.join("claude"));
        dir
    }

    /// Links the fake `claude` into `claude-bin/` and returns that dir; the config does not
    /// name it.
    pub(crate) fn link_fake_claude(&self) -> PathBuf {
        let dir = self.path("claude-bin");
        std::fs::create_dir(&dir).unwrap();
        let script = self.path("fake-claude");
        moor_testkit::write_executable(&script, FAKE_CLAUDE).unwrap();
        std::os::unix::fs::symlink(&script, dir.join("claude")).unwrap();
        dir
    }

    /// Makes run `run` of the fake `claude` in `bin`, or every run when `None`, call these
    /// tools of the front's tool server, in order, each as `(name, arguments)`.
    pub(crate) fn tool_calls(bin: &Path, run: Option<u32>, calls: &[(&str, serde_json::Value)]) {
        let mut lines = String::new();
        for (i, (name, args)) in calls.iter().enumerate() {
            let call = serde_json::json!({
                "jsonrpc": "2.0",
                "id": i + 2,
                "method": "tools/call",
                "params": {"name": name, "arguments": args},
            });
            lines.push_str(&call.to_string());
            lines.push('\n');
        }
        let file = run.map_or_else(|| "calls".to_owned(), |n| format!("calls-{n}"));
        std::fs::write(bin.join(file), lines).unwrap();
    }

    /// The tool results run `run` of the fake `claude` in `bin` got, in call order, as
    /// `(text, is_error)`.
    pub(crate) fn tool_results(bin: &Path, run: u32) -> Vec<(String, bool)> {
        let out = std::fs::read_to_string(bin.join(format!("run-{run}")).join("mcp")).unwrap();
        out.lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["id"].as_u64().is_some_and(|id| id >= 2))
            .map(|v| {
                let result = &v["result"];
                let text = result["content"][0]["text"].as_str().unwrap_or_default();
                (
                    text.to_owned(),
                    result["isError"].as_bool().unwrap_or(false),
                )
            })
            .collect()
    }

    /// Polls Slack until some post in the thread reads `text`; fails after ten seconds.
    pub(crate) fn wait_post(&self, text: &str) {
        for _ in 0..500 {
            if self.slack.posts().iter().any(|p| p.text == text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("no post reads {text:?}: {:?}", self.slack.posts());
    }

    /// Polls `slack.db` until `n` coordinator turns have their line posted; fails after ten
    /// seconds.
    pub(crate) fn wait_replies(&self, n: usize) {
        let db = rusqlite::Connection::open(self.path("state-slack/slack.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let sql = "SELECT COUNT(*) FROM slack_turns WHERE posted_at IS NOT NULL";
        for _ in 0..500 {
            let posted = db.query_row(sql, [], |r| r.get::<_, i64>(0));
            if posted.is_ok_and(|posted| posted >= i64::try_from(n).unwrap_or(i64::MAX)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("fewer than {n} turn lines were posted");
    }

    /// Polls `slack.db` until Box `name` is in `state`; fails after ten seconds.
    pub(crate) fn wait_box_state(&self, name: &str, state: &str) {
        let db = rusqlite::Connection::open(self.path("state-slack/slack.db")).unwrap();
        for _ in 0..500 {
            let found: Option<String> = db
                .query_row(
                    "SELECT state FROM slack_boxes WHERE box_name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .ok();
            if found.as_deref() == Some(state) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Box {name} never reached {state}");
    }

    /// Has the scripted controller lease its front Proxy to `user` with `token` as the lease,
    /// as it does once `/moor login` stored a Claude token; the fake `claude` answers by it.
    pub(crate) fn link_token(&self, user: &str, token: &str) {
        let dir = self.path("leases");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        secret_file(&dir.join(user), token);
    }

    /// A scripted controller and a daemon whose coordinator is the fake `claude`, with
    /// `user`'s token linked when given, connected to Slack.
    pub(crate) fn start_coordinated(token: Option<&str>) -> (Self, FakeController, PathBuf) {
        Self::start_coordinated_by(token, Self::spawn_daemon)
    }

    /// Like [`Self::start_coordinated`], with `spawn` starting the daemon.
    fn start_coordinated_by(
        token: Option<&str>,
        spawn: fn(&mut Self),
    ) -> (Self, FakeController, PathBuf) {
        let mut rig = Self::new();
        let bin = rig.use_fake_claude(None);
        if let Some(token) = token {
            rig.link_token(USER, token);
        }
        let controller = rig.serve(PROTOCOL);
        spawn(&mut rig);
        rig.slack.wait_connected();
        (rig, controller, bin)
    }

    /// A scripted controller that keeps Boxes, and a daemon whose fake `claude` calls
    /// `start_box` with prompt `fix the thing` in its first turn, connected to Slack with
    /// [`REPO`] as the channel's default. Later turns call what the test writes.
    pub(crate) fn start_boxes() -> (Self, FakeController, PathBuf) {
        Self::start_boxes_by(Self::spawn_daemon)
    }

    /// Like [`Self::start_boxes`], with the daemon given its socket the way the controller
    /// starts a front: an `O_PATH` descriptor on fd 3 that `MOOR_FRONT_SOCKET` names.
    pub(crate) fn start_boxes_as_front() -> (Self, FakeController, PathBuf) {
        Self::start_boxes_by(Self::spawn_daemon_as_front)
    }

    /// Like [`Self::start_boxes`], with `spawn` starting the daemon.
    fn start_boxes_by(spawn: fn(&mut Self)) -> (Self, FakeController, PathBuf) {
        let (rig, controller, bin) = Self::start_coordinated_by(Some(MCP), spawn);
        controller.keep_boxes();
        rig.set_default_repo(REPO);
        let prompt = serde_json::json!({"prompt": "fix the thing"});
        Self::tool_calls(&bin, Some(0), &[("start_box", prompt)]);
        Self::tool_calls(&bin, None, &[]);
        (rig, controller, bin)
    }

    /// Opens thread [`TS`] with a mention and waits until its first Box is bound.
    pub(crate) fn open_thread_with_a_box(&self) {
        self.mention(1, USER, "<@UBOT> fix the thing", TS, None);
        self.wait_post(&started_line(BOX_NAME));
        self.wait_box_state(BOX_NAME, "bound");
    }

    /// Adds `repo` to the test channel as its default, as `/moor repo` does.
    pub(crate) fn set_default_repo(&self, repo: &str) {
        let db = rusqlite::Connection::open(self.path("state-slack/slack.db")).unwrap();
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        db.execute(
            "INSERT INTO slack_channel_repos (channel, repo, added_by, added_at, is_default) \
             VALUES (?1, ?2, ?3, 0, 1)",
            [CHANNEL, repo, USER],
        )
        .unwrap();
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

    /// The controller's `slack` front socket, which a real controller serves.
    pub(crate) fn front_socket(&self) -> PathBuf {
        front_socket(self.root.path())
    }

    /// Spawns the daemon on the real controller's front socket, with its state dir in the
    /// rig's root.
    pub(crate) fn spawn_daemon_on_front(&mut self) {
        let (socket, state) = (self.front_socket(), self.path("state-slack"));
        self.spawn_daemon_without_socket(|daemon| {
            daemon
                .arg("--socket")
                .arg(socket)
                .arg("--state-dir")
                .arg(state);
        });
    }

    /// Starts a real controller that runs the daemon as its `slack` front, writing only the
    /// returned dir, outside /tmp, which the front sees as its own tmp dir. The config and token
    /// files are copied there: the front sees nothing of the rig's root.
    pub(crate) fn real_controller_running_the_daemon(&self) -> (RealController, tempfile::TempDir) {
        let dir = moor_testkit::short_tempdir("ms").unwrap();
        let state = dir.path().to_path_buf();
        secret_file(&state.join("app.token"), APP_TOKEN);
        secret_file(&state.join("bot.token"), BOT_TOKEN);
        let config = std::fs::read_to_string(self.path("slack.yaml"))
            .unwrap()
            .replace(
                &*self.root.path().to_string_lossy(),
                &state.to_string_lossy(),
            );
        std::fs::write(state.join("slack.yaml"), config).unwrap();
        let command = serde_json::json!([
            env!("CARGO_BIN_EXE_moor-slack"),
            "--config",
            state.join("slack.yaml"),
        ]);
        let entry =
            serde_json::json!({"command": command, "state_dir": state, "proxy_rules": ["claude"]});
        let controller =
            RealController::start_with(self.root.path(), &self.socket(), &entry.to_string());
        (controller, dir)
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
        let socket = self.socket();
        self.spawn_daemon_without_socket(|daemon| {
            daemon.arg("--socket").arg(socket);
            extra(daemon);
        });
    }

    /// Spawns the daemon with its state dir in the rig's root and the rig's socket on fd 3 as
    /// an `O_PATH` descriptor, named only by `MOOR_FRONT_SOCKET=/proc/self/fd/3`.
    pub(crate) fn spawn_daemon_as_front(&mut self) {
        use command_fds::{CommandFdExt as _, FdMapping};
        use rustix::fs::{Mode, OFlags};
        let socket =
            rustix::fs::open(self.socket(), OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).unwrap();
        let state = self.path("state-slack");
        self.spawn_daemon_without_socket(|daemon| {
            daemon
                .arg("--state-dir")
                .arg(state)
                .env("MOOR_FRONT_SOCKET", "/proc/self/fd/3")
                .fd_mappings(vec![FdMapping {
                    parent_fd: socket,
                    child_fd: 3,
                }])
                .unwrap();
        });
    }

    /// Like [`Self::spawn_daemon_with`], but `--socket` is left to `extra`.
    pub(crate) fn spawn_daemon_without_socket(&mut self, extra: impl FnOnce(&mut Command)) {
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_moor-slack"));
        daemon
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path("home"))
            .env("MOOR_FRONT_PROXY_PORT", "1")
            .env("MOOR_FRONT_PROXY_CA_PEM", "fake pem")
            .arg("--config")
            .arg(self.path("slack.yaml"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.path("slack.err")).unwrap());
        extra(&mut daemon);
        self.daemon = Some(daemon.spawn().unwrap());
    }

    /// Sends SIGTERM to the daemon's exact pid.
    pub(crate) fn terminate_daemon(&self) {
        let pid = self.daemon.as_ref().unwrap().id();
        let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
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

    /// Pushes a mention in the test channel from `user`, with event and envelope ids derived from `n`.
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
