//! End-to-end tests for the controller's Slack app, driven through an in-process fake Slack.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use super::fresh_dir;
use super::slack_fake::{FakeSlack, Mention, APP_TOKEN, BOT_TOKEN};

/// Channel the controller is configured to answer in.
pub(crate) const CHANNEL: &str = "C0ALLOWED";
/// User the controller is configured to obey.
pub(crate) const USER: &str = "U0ALLOWED";
/// Ts of the first mention in most tests.
pub(crate) const TS: &str = "1730000000.000100";
/// Task name the first mention maps to.
pub(crate) const TASK: &str = "slack-1730000000-000100";
/// Pause that lets a wrongly emitted second reply arrive before the test counts replies.
pub(crate) const QUIET: Duration = Duration::from_millis(600);

/// A running controller wired to a fake Slack, killed on drop.
pub(crate) struct Rig {
    /// Root holding home, state, tokens and the socket.
    pub(crate) root: PathBuf,
    /// The fake Slack the controller talks to.
    pub(crate) fake: FakeSlack,
    /// The controller process.
    child: Child,
    /// Controller socket path.
    socket: PathBuf,
}

/// Writes `text` to `path` readable by its owner only.
fn secret_file(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// Creates a one-commit git repo at `dir`.
fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README"), "demo\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
}

impl Rig {
    pub(crate) fn start() -> Self {
        let root = fresh_dir("slack");
        let fake = FakeSlack::start();
        let (app, bot) = (root.join("app.token"), root.join("bot.token"));
        secret_file(&app, APP_TOKEN);
        secret_file(&bot, BOT_TOKEN);
        let repo = root.join("demo-repo");
        make_repo(&repo);
        // Kept short: Unix socket paths are limited to about 104 bytes.
        let socket = root.join("c.sock");
        let config = root.join("c.yaml");
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\nrepos:\n  demo:\n    url: {}\nslack:\n  app_token_file: {}\n  bot_token_file: {}\n  channels: [{CHANNEL}]\n  users: [{USER}]\n  api_base: {}\n",
                socket.display(),
                root.join("state").display(),
                repo.display(),
                app.display(),
                bot.display(),
                fake.api_base(),
            ),
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_moor"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MOOR_STATE_DIR", root.join("state"))
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let rig = Self {
            root,
            fake,
            child,
            socket,
        };
        rig.fake.wait_connected();
        rig
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
        self.fake.send_mention(&Mention {
            event_id: &format!("Ev{n}"),
            envelope_id: &format!("env-{n}"),
            channel: CHANNEL,
            user,
            text,
            ts,
            thread_ts,
        });
    }

    /// The `show` envelope for `task` as the controller socket answers it.
    pub(crate) fn show(&self, task: &str) -> Value {
        let writer = UnixStream::connect(&self.socket).unwrap();
        writer
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(writer.try_clone().unwrap());
        let mut writer = writer;
        let mut exchange = |line: String| {
            writeln!(writer, "{line}").unwrap();
            let mut reply = String::new();
            reader.read_line(&mut reply).unwrap();
            serde_json::from_str::<Value>(&reply).unwrap()
        };
        exchange(
            json!({"frame": "hello", "protocol": moor_core::controller::PROTOCOL}).to_string(),
        );
        let request =
            json!({"frame": "request", "id": "1", "request": {"cmd": "show", "task": task}});
        exchange(request.to_string())["envelope"].clone()
    }

    /// True when the controller knows a Task called `task`.
    pub(crate) fn has_task(&self, task: &str) -> bool {
        self.show(task)["class"] != "failed"
    }

    /// Kills the controller and returns everything it wrote to stderr.
    pub(crate) fn stderr(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut text = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_string(&mut text).unwrap();
        }
        text
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn allowed_mention_starts_a_task_and_replies_in_the_thread() {
    let rig = Rig::start();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    assert_eq!(rig.fake.wait_acks(1), ["env-1"]);
    let posts = rig.fake.wait_posts(1);
    assert_eq!(posts[0].channel, CHANNEL);
    assert_eq!(posts[0].thread_ts, TS);
    assert!(posts[0].text.contains(TASK), "{}", posts[0].text);
    let requests = rig.fake.requests();
    assert!(requests
        .iter()
        .filter(|r| r.path == "/apps.connections.open")
        .all(|r| r.auth == format!("Bearer {APP_TOKEN}")));
    assert!(requests
        .iter()
        .filter(|r| r.path == "/chat.postMessage")
        .all(|r| r.auth == format!("Bearer {BOT_TOKEN}")));
}

#[test]
fn redelivered_event_is_handled_once() {
    let rig = Rig::start();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.fake.send_mention(&Mention {
        event_id: "Ev1",
        envelope_id: "env-1-retry",
        channel: CHANNEL,
        user: USER,
        text: "<@UBOT> start demo fix the thing",
        ts: TS,
        thread_ts: None,
    });
    assert_eq!(rig.fake.wait_acks(2), ["env-1", "env-1-retry"]);
    rig.fake.wait_posts(1);
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.posts().len(), 1, "{:?}", rig.fake.posts());
}

#[test]
fn second_mention_in_a_thread_starts_nothing_new() {
    let rig = Rig::start();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.fake.wait_posts(1);
    rig.mention(
        2,
        USER,
        "<@UBOT> start demo and another thing",
        "1730000005.000200",
        Some(TS),
    );
    let posts = rig.fake.wait_posts(2);
    assert_eq!(posts[1].thread_ts, TS);
    assert!(posts[1].text.contains(TASK), "{}", posts[1].text);
    assert!(!posts[1].text.contains("000200"), "{}", posts[1].text);
    assert!(!rig.has_task("slack-1730000005-000200"));
}

#[test]
fn user_off_the_allow_list_is_refused_and_nothing_starts() {
    let rig = Rig::start();
    rig.mention(1, "U0OTHER", "<@UBOT> start demo fix the thing", TS, None);
    let posts = rig.fake.wait_posts(1);
    assert_eq!(posts[0].thread_ts, TS);
    assert!(posts[0].text.contains("<@U0OTHER>"), "{}", posts[0].text);
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.posts().len(), 1);
    assert!(!rig.has_task(TASK));
}

#[test]
fn tokens_never_reach_stderr() {
    let mut rig = Rig::start();
    rig.mention(1, USER, "<@UBOT> start demo fix the thing", TS, None);
    rig.fake.wait_posts(1);
    let stderr = rig.stderr();
    assert!(!stderr.contains(APP_TOKEN), "{stderr}");
    assert!(!stderr.contains(BOT_TOKEN), "{stderr}");
}
