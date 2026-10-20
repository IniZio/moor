//! `/moor login` and `/moor logout` against the daemon, with a fake `claude` that records the
//! argv, env, stdin and fd 3 it got.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::controller::PROTOCOL;
use serde_json::Value;

use super::support::{Rig, CHANNEL, QUIET, USER};

/// A made-up token the fake accepts. It is no real credential.
const GOOD: &str = "sk-ant-oat01-fake-good-3c9a1f07d2e84b65";

/// How long a fake check may take.
const CHECK_WAIT: Duration = Duration::from_secs(10);

/// The modal text after a token is linked.
const LINKED: &str = "Linked. Your token pays for the coordinator in threads you start.";

/// A rig whose daemon runs the fake `claude`; returns the rig and the fake's directory.
fn start(version: Option<&str>) -> (Rig, PathBuf) {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(version);
    let _controller = rig.serve(PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    (rig, bin)
}

/// Runs `/moor login` as [`USER`], waits for the modal, then submits `token` in it; returns
/// the text of the modal once the worker updated it.
fn link(rig: &Rig, token: &str) -> String {
    rig.slack.send_slash("env-1", CHANNEL, USER, "login");
    let opened = rig.slack.wait_views("views.open", 1, CHECK_WAIT);
    assert_eq!(opened[0]["trigger_id"], "trigger-env-1");
    assert_eq!(opened[0]["view"]["callback_id"], "moor_login");
    rig.slack
        .send_submission("env-2", USER, &opened[0]["view"], token);
    let ack = rig.slack.wait_ack_payload("env-2");
    assert_eq!(ack["response_action"], "update");
    assert_eq!(modal_text(&ack["view"]), "Checking your token…");
    let updated = rig.slack.wait_views("views.update", 1, CHECK_WAIT);
    assert_eq!(updated[0]["view_id"], "V1");
    modal_text(&updated[0]["view"]).to_owned()
}

fn modal_text(view: &Value) -> &str {
    view["blocks"][0]["text"]["text"].as_str().unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn recorded(bin: &Path, file: &str) -> String {
    std::fs::read_to_string(bin.join("run-0").join(file)).unwrap()
}

/// True when any file in `dir` holds `needle`.
fn any_file_holds(dir: &Path, needle: &str) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        let bytes = if path.is_file() {
            std::fs::read(&path).unwrap()
        } else {
            Vec::new()
        };
        bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
    })
}

#[test]
fn a_linked_token_reaches_claude_on_fd_3_only() {
    let (rig, bin) = start(None);
    assert_eq!(link(&rig, GOOD), LINKED);
    let state = rig.path("state-slack");
    assert_eq!(recorded(&bin, "fd3"), GOOD);
    for file in ["argv", "env", "stdin", "cwd"] {
        assert!(
            !recorded(&bin, file).contains(GOOD),
            "the token reached {file}"
        );
    }
    let mut env: Vec<String> = recorded(&bin, "env").lines().map(str::to_owned).collect();
    env.sort();
    let want = [
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3".to_owned(),
        format!(
            "CLAUDE_CONFIG_DIR={}",
            state.join("claude").join(USER).display()
        ),
        format!("HOME={}", rig.path("home").display()),
        "LANG=C.UTF-8".to_owned(),
        "PATH=/usr/local/bin:/usr/bin:/bin".to_owned(),
    ];
    assert_eq!(env, want);
    let fds = recorded(&bin, "fds");
    assert!(!fds.contains("slack.db"), "{fds}");
    assert!(!fds.contains("socket:"), "{fds}");
}

#[test]
fn a_linked_token_is_stored_owner_only_and_never_in_slack_db_or_the_log() {
    let (rig, _bin) = start(None);
    assert_eq!(link(&rig, GOOD), LINKED);
    let state = rig.path("state-slack");
    let file = state.join("credentials/claude").join(USER);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), GOOD);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&state.join("credentials/claude")), 0o700);
    assert_eq!(mode(&state.join("claude").join(USER)), 0o700);
    assert!(!any_file_holds(&state, GOOD), "the token reached slack.db");
    let stderr = rig.stderr();
    assert!(
        stderr.contains("coordinator: Claude Code 2.1.304"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("login of {USER}: Linked.")),
        "{stderr}"
    );
    assert!(!stderr.contains(GOOD), "the token reached the log");
}

#[test]
fn a_token_claude_refuses_is_not_stored() {
    let (rig, _bin) = start(None);
    let text = link(&rig, "sk-ant-oat01-made-up-8d1e");
    let want = "Claude refused this token: Failed to authenticate. API Error: 401 OAuth \
                access token is invalid. Nothing was stored.";
    assert_eq!(text, want);
    assert!(!rig
        .path("state-slack/credentials/claude")
        .join(USER)
        .exists());
    assert!(!rig.stderr().contains("sk-ant-oat01-made-up-8d1e"));
}

#[test]
fn an_old_claude_code_is_refused_at_start_and_checks_no_token() {
    let (rig, bin) = start(Some("2.1.303 (Claude Code)"));
    let stderr = rig.stderr();
    let line = "coordinator unavailable: Claude Code 2.1.303 is older than 2.1.304";
    assert!(stderr.contains(line), "{stderr}");
    let text = link(&rig, GOOD);
    let want = "The coordinator is unavailable: Claude Code 2.1.303 is older than 2.1.304. \
                Nothing was stored.";
    assert_eq!(text, want);
    assert!(!bin.join("run-0").exists(), "no check ran");
    assert!(!rig
        .path("state-slack/credentials/claude")
        .join(USER)
        .exists());
}

#[test]
fn a_user_off_the_list_gets_no_modal_and_stores_nothing() {
    let (rig, bin) = start(None);
    rig.slack.send_slash("env-1", CHANNEL, "U0OTHER", "login");
    let ack = rig.slack.wait_ack_payload("env-1");
    let text = "<@U0OTHER> is not allowed to link a token here.";
    assert_eq!(ack["text"], text);
    let view = serde_json::json!({"callback_id": "moor_login", "private_metadata": "claude"});
    rig.slack.send_submission("env-2", "U0OTHER", &view, GOOD);
    let ack = rig.slack.wait_ack_payload("env-2");
    let text = "You are not allowed to link a token here. Nothing was stored.";
    assert_eq!(modal_text(&ack["view"]), text);
    std::thread::sleep(QUIET);
    assert!(rig.slack.views("views.open").is_empty());
    assert!(rig.slack.views("views.update").is_empty());
    assert!(!bin.join("run-0").exists());
}

#[test]
fn logout_deletes_the_token_file() {
    let (rig, _bin) = start(None);
    assert_eq!(link(&rig, GOOD), LINKED);
    rig.slack.send_slash("env-3", CHANNEL, USER, "logout");
    assert_eq!(rig.slack.wait_ack_payload("env-3")["text"], "Unlinked.");
    assert!(!rig
        .path("state-slack/credentials/claude")
        .join(USER)
        .exists());
}

#[test]
#[ignore = "runs real Claude Code; set MOOR_SLACK_LIVE_CLAUDE and MOOR_SLACK_LIVE_TOKEN_FILE"]
fn a_real_claude_code_links_a_real_token() {
    let binary = std::env::var_os("MOOR_SLACK_LIVE_CLAUDE").expect("MOOR_SLACK_LIVE_CLAUDE");
    let token_file =
        std::env::var_os("MOOR_SLACK_LIVE_TOKEN_FILE").expect("MOOR_SLACK_LIVE_TOKEN_FILE");
    let token = std::fs::read_to_string(token_file).expect("read the token file");
    let mut rig = Rig::new();
    rig.set_coordinator(Path::new(&binary));
    let _controller = rig.serve(PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.slack.send_slash("env-1", CHANNEL, USER, "login");
    let opened = rig.slack.wait_views("views.open", 1, CHECK_WAIT);
    rig.slack
        .send_submission("env-2", USER, &opened[0]["view"], token.trim());
    let updated = rig
        .slack
        .wait_views("views.update", 1, Duration::from_secs(120));
    assert_eq!(modal_text(&updated[0]["view"]), LINKED);
    assert!(
        !rig.stderr().contains(token.trim()),
        "the token reached the log"
    );
}
