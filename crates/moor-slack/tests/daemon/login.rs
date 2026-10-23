//! `/moor login` and `/moor logout` against the daemon: a fake `claude` that records the argv,
//! env, stdin and fd 3 it got, a fake GitHub API, and a scripted controller that records the
//! requests the front sends as the Slack user who logged in.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::controller::PROTOCOL;
use moor_core::request::Request;
use serde_json::Value;

use super::support::controller::{failed, FakeController};
use super::support::slack_fake::TEAM;
use super::support::{Rig, CHANNEL, QUIET, TS, USER};

/// A made-up token the fake accepts. It is no real credential.
const GOOD: &str = "sk-ant-oat01-fake-good-3c9a1f07d2e84b65";

/// How long a fake check may take.
const CHECK_WAIT: Duration = Duration::from_secs(10);

/// A made-up GitHub token the fake accepts. It is no real credential.
const GH_GOOD: &str = "gho_fake_good_7f31c2a9d4e86b05";

/// The modal text after a token is linked.
const LINKED: &str = "Linked.";

/// A rig whose daemon runs the fake `claude`; returns the rig, the fake's directory and the
/// scripted controller.
fn start(version: Option<&str>) -> (Rig, PathBuf, FakeController) {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(version);
    let controller = rig.serve(PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    (rig, bin, controller)
}

/// Runs `/moor login` as [`USER`], waits for the modal, then submits `token` in it; returns
/// the text of the modal once the worker updated it.
fn link(rig: &Rig, token: &str) -> String {
    link_account(rig, "login", token)
}

/// Like [`link`], with `command` as the text of the slash command, such as `login github`.
fn link_account(rig: &Rig, command: &str, token: &str) -> String {
    link_as(rig, USER, command, token)
}

/// Like [`link_account`], for the Slack user `user`.
fn link_as(rig: &Rig, user: &str, command: &str, token: &str) -> String {
    rig.slack.send_slash("env-1", CHANNEL, user, command);
    let opened = rig.slack.wait_views("views.open", 1, CHECK_WAIT);
    assert_eq!(opened[0]["trigger_id"], "trigger-env-1");
    assert_eq!(opened[0]["view"]["callback_id"], "moor_login");
    rig.slack
        .send_submission("env-2", user, &opened[0]["view"], token);
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

/// The requests the controller got as `(identity, command)`, in order, without the plain ones
/// the front sends for its own turns.
fn asked(controller: &FakeController) -> Vec<(String, &'static str)> {
    controller.seen(|seen| {
        let who = seen.acting.iter().map(|w| w.clone().unwrap_or_default());
        who.zip(&seen.requests)
            .filter_map(|(who, request)| match request {
                Request::Enroll(_) => Some((who, "enroll")),
                Request::SecretSet(_) => Some((who, "secret-set")),
                Request::SecretDelete(_) => Some((who, "secret-delete")),
                _ => None,
            })
            .collect()
    })
}

/// The `(name, value)` of each `secret-set` the controller got.
fn secrets_set(controller: &FakeController) -> Vec<(String, String)> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|request| match request {
                Request::SecretSet(set) => {
                    let value = String::from_utf8_lossy(set.value.secret().expose()).into_owned();
                    Some((set.name.as_str().to_owned(), value))
                }
                _ => None,
            })
            .collect()
    })
}

fn identity(user: &str) -> String {
    format!("slack:{TEAM}/{user}")
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
fn a_linked_claude_token_reaches_claude_on_fd_3_only() {
    let (rig, bin, _controller) = start(None);
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
        "CLAUDE_CODE_DISABLE_CLAUDE_MDS=1".to_owned(),
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
fn a_claude_token_claude_takes_goes_to_the_vault_as_that_user_and_no_file_is_written() {
    let (rig, _bin, controller) = start(None);
    assert_eq!(link(&rig, GOOD), LINKED);
    let who = identity(USER);
    assert_eq!(
        asked(&controller),
        [(who.clone(), "enroll"), (who, "secret-set")]
    );
    assert_eq!(
        secrets_set(&controller),
        [("claude".to_owned(), GOOD.to_owned())]
    );
    let state = rig.path("state-slack");
    assert!(
        !state.join("credentials").exists(),
        "login wrote the coordinator's credential file"
    );
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
    let (rig, _bin, controller) = start(None);
    let text = link(&rig, "sk-ant-oat01-made-up-8d1e");
    let want = "Claude refused this token: Failed to authenticate. API Error: 401 OAuth \
                access token is invalid. Nothing was stored.";
    assert_eq!(text, want);
    assert_eq!(asked(&controller), []);
    assert!(!rig.path("state-slack/credentials").exists());
    assert!(!rig.stderr().contains("sk-ant-oat01-made-up-8d1e"));
}

#[test]
fn an_old_claude_code_is_refused_at_start_and_checks_no_token() {
    let (rig, bin, controller) = start(Some("2.1.303 (Claude Code)"));
    let stderr = rig.stderr();
    let line = "coordinator unavailable: Claude Code 2.1.303 is older than 2.1.304";
    assert!(stderr.contains(line), "{stderr}");
    let text = link(&rig, GOOD);
    let want = "The coordinator is unavailable: Claude Code 2.1.303 is older than 2.1.304. \
                Nothing was stored.";
    assert_eq!(text, want);
    assert!(!bin.join("run-0").exists(), "no check ran");
    assert_eq!(asked(&controller), []);
}

#[test]
fn a_github_token_github_takes_goes_to_the_vault_as_that_user() {
    let (rig, _bin, controller) = start(None);
    assert_eq!(link_account(&rig, "login github", GH_GOOD), LINKED);
    let who = identity(USER);
    assert_eq!(
        asked(&controller),
        [(who.clone(), "enroll"), (who, "secret-set")]
    );
    assert_eq!(
        secrets_set(&controller),
        [("github".to_owned(), GH_GOOD.to_owned())]
    );
    let state = rig.path("state-slack");
    assert!(!state.join("credentials").exists());
    assert!(
        !any_file_holds(&state, GH_GOOD),
        "the token reached slack.db"
    );
    assert!(!rig.stderr().contains(GH_GOOD), "the token reached the log");
}

#[test]
fn a_github_token_is_checked_with_one_get_user_as_a_bearer_with_a_user_agent() {
    let (rig, _bin, _controller) = start(None);
    assert_eq!(link_account(&rig, "login github", GH_GOOD), LINKED);
    let asked = rig.github.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].line.starts_with("GET /user "), "{asked:?}");
    assert_eq!(asked[0].auth, format!("Bearer {GH_GOOD}"));
    assert!(!asked[0].user_agent.is_empty(), "{asked:?}");
}

#[test]
fn a_github_token_github_refuses_stores_nothing_and_the_modal_says_so() {
    for status in [401, 403] {
        let (rig, _bin, controller) = start(None);
        rig.github.answer(status);
        let text = link_account(&rig, "login github", GH_GOOD);
        let want = format!(
            "GitHub refused this token: GitHub answered HTTP {status} for it. Nothing was stored."
        );
        assert_eq!(text, want);
        assert_eq!(asked(&controller), [], "{status}");
        assert!(!rig.stderr().contains(GH_GOOD));
    }
}

#[test]
fn a_github_that_cannot_answer_stores_nothing_and_says_it_is_unavailable() {
    let (rig, _bin, controller) = start(None);
    rig.github.answer(503);
    let text = link_account(&rig, "login github", GH_GOOD);
    assert_eq!(
        text,
        "GitHub is unavailable: GitHub answered HTTP 503. Nothing was stored."
    );
    assert_eq!(asked(&controller), []);
}

#[test]
fn a_pasted_github_token_with_a_space_in_it_is_refused_before_github_is_asked() {
    let (rig, _bin, controller) = start(None);
    let text = link_account(&rig, "login github", "gho_a b");
    let want = "GitHub refused this token: it is not the one word `gh auth token` prints. \
                Nothing was stored.";
    assert_eq!(text, want);
    assert!(rig.github.asked().is_empty());
    assert_eq!(asked(&controller), []);
}

#[test]
fn a_vault_that_refuses_the_secret_is_reported_and_the_token_stays_out_of_the_log() {
    let (rig, _bin, controller) = start(None);
    let refusal = failed("secret-set", "the vault is sealed");
    controller.script(
        "secret-set",
        super::support::controller::Scripted::Reply(refusal),
    );
    let text = link_account(&rig, "login github", GH_GOOD);
    let want = "Could not store your token: the vault is sealed. Nothing was stored.";
    assert_eq!(text, want);
    assert!(!rig.stderr().contains(GH_GOOD));
}

#[test]
fn a_user_who_was_never_listed_logs_in_and_so_does_a_second_user_as_themselves() {
    let (rig, _bin, controller) = start(None);
    assert_eq!(link_as(&rig, "U0OTHER", "login github", GH_GOOD), LINKED);
    let who = identity("U0OTHER");
    assert_eq!(
        asked(&controller),
        [(who.clone(), "enroll"), (who, "secret-set")]
    );
}

#[test]
fn logout_deletes_the_named_secret_as_that_user_and_the_answer_says_so() {
    let (rig, _bin, controller) = start(None);
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "logout github");
    let ack = rig.slack.wait_ack_payload("env-3");
    assert_eq!(ack["text"], "Unlinking your GitHub token…");
    let answers = rig.slack.wait_responses(1);
    assert_eq!(
        answers,
        [("env-3".to_owned(), "Unlinked your GitHub token.".to_owned())]
    );
    assert_eq!(asked(&controller), [(identity(USER), "secret-delete")]);
    let named = controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::SecretDelete(d) => Some(d.name.as_str().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(named, ["github"]);
    rig.slack.send_slash("env-4", CHANNEL, USER, "logout");
    rig.slack.wait_responses(2);
    let named = controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::SecretDelete(d) => Some(d.name.as_str().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(named, ["github", "claude"]);
}

#[test]
fn a_logout_the_controller_refuses_says_why_to_the_user() {
    let (rig, _bin, controller) = start(None);
    let refusal = failed("secret-delete", "log in first");
    controller.script(
        "secret-delete",
        super::support::controller::Scripted::Reply(refusal),
    );
    rig.slack
        .send_slash("env-3", CHANNEL, USER, "logout claude");
    let answers = rig.slack.wait_responses(1);
    assert_eq!(
        answers[0].1,
        "Could not unlink your Claude token: log in first"
    );
}

#[test]
fn a_user_nobody_listed_mentions_the_bot_in_an_allowed_channel_and_a_thread_opens() {
    let (rig, _bin, _controller) = start(None);
    rig.mention(1, "U0NEWBIE", "<@UBOT> hello", TS, None);
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let owner = |db: &rusqlite::Connection| -> Option<String> {
        db.query_row("SELECT owner FROM slack_threads", [], |r| r.get(0))
            .ok()
    };
    let mut found = None;
    for _ in 0..500 {
        found = owner(&db);
        if found.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(found.as_deref(), Some("U0NEWBIE"));
    assert!(!rig
        .slack
        .posts()
        .iter()
        .any(|p| p.text.contains("not allowed")));
}

#[test]
fn a_mention_in_a_channel_that_is_not_allowed_is_still_ignored() {
    let (rig, _bin, controller) = start(None);
    rig.slack
        .send_mention(&super::support::slack_fake::Mention {
            event_id: "Ev1",
            envelope_id: "env-1",
            channel: "C0ELSEWHERE",
            user: USER,
            text: "<@UBOT> hello",
            ts: TS,
            thread_ts: None,
        });
    rig.slack.wait_acks(1);
    std::thread::sleep(QUIET);
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let threads: i64 = db
        .query_row("SELECT COUNT(*) FROM slack_threads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(threads, 0);
    assert!(rig.slack.posts().is_empty());
    assert_eq!(controller.seen(|seen| seen.requests.len()), 0);
}

#[test]
fn a_redelivered_login_opens_one_modal() {
    let (rig, _bin, _controller) = start(None);
    rig.slack.send_slash("env-1", CHANNEL, USER, "login github");
    rig.slack.send_slash("env-1", CHANNEL, USER, "login github");
    rig.slack.wait_views("views.open", 1, CHECK_WAIT);
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.views("views.open").len(), 1);
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
