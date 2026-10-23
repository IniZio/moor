//! Tests for `/moor login`, `/moor logout`, the submitted modal, the token checks and what
//! the worker asks of the controller.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code reads JSON by key and fails on a wrong request"
)]

use std::cell::{Cell, RefCell};
use std::path::Path;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use moor_core::secret::Secret;
use serde_json::{json, Value};

use super::{form, login, logout, submission, vault, Checks, TokenCheck, CHECKING};
use crate::account::Account;
use crate::coordinator::claude::tests::install;
use crate::coordinator::claude::{Claude, HostEnv};
use crate::credentials::{CheckError, CredentialCheck, SlackUser};
use crate::db::Db;
use crate::github::GitHub;
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

fn db(dir: &Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

fn slash(envelope_id: &str, user: &str, text: &str) -> Slash {
    Slash {
        envelope_id: envelope_id.into(),
        channel: "C1".into(),
        user: user.into(),
        text: text.into(),
        trigger_id: Some(format!("trig-{envelope_id}")),
        response_url: Some(format!("https://hooks.example/{envelope_id}")),
    }
}

fn submitted(envelope_id: &str, user: &str, account: &str, token: &str) -> TokenSubmission {
    TokenSubmission {
        envelope_id: envelope_id.into(),
        user: user.into(),
        view_id: "V1".into(),
        account: account.into(),
        token: Secret::new(token.into()),
    }
}

fn ack_text(kept: &Kept) -> &str {
    kept.ack.as_ref().unwrap()["text"].as_str().unwrap()
}

fn modal_text(ack: &Value) -> &str {
    assert_eq!(ack["response_action"], "update");
    ack["view"]["blocks"][0]["text"]["text"].as_str().unwrap()
}

fn user(id: &str) -> SlackUser {
    SlackUser::parse(id).unwrap()
}

#[test]
fn login_opens_the_modal_for_the_named_account_once_after_an_empty_ack() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let cases = [
        ("login", Account::Claude),
        ("login claude", Account::Claude),
        ("login github", Account::GitHub),
    ];
    for (n, (text, account)) in cases.into_iter().enumerate() {
        let id = format!("e{n}");
        let kept = login(&slash(&id, "U1", text), &db, 1).unwrap();
        assert_eq!(kept.ack, None);
        let open = AfterAck::OpenLogin {
            trigger_id: format!("trig-{id}"),
            account,
        };
        assert_eq!(kept.then, Some(open), "{text}");
    }
    let again = login(&slash("e0", "U1", "login"), &db, 2).unwrap();
    assert_eq!(
        again.then, None,
        "a redelivered command opens no second modal"
    );
    let modal = form(Account::Claude);
    assert_eq!(modal["private_metadata"], "claude");
    let prompt = "Paste the token that `claude setup-token` printed on your machine.";
    assert_eq!(modal["blocks"][0]["label"]["text"], prompt);
    let modal = form(Account::GitHub);
    assert_eq!(modal["private_metadata"], "github");
    let prompt = "Paste the output of `gh auth token` from your machine.";
    assert_eq!(modal["blocks"][0]["label"]["text"], prompt);
}

#[test]
fn login_refuses_an_unknown_account_extra_words_or_a_bad_id_and_asks_no_list() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let cases = [
        (
            "U1",
            "login codex",
            "moor cannot link that. It can link `claude` and `github`.",
        ),
        (
            "U1",
            "login claude extra",
            "Usage: `/moor login [claude|github]` or `/moor logout [claude|github]`.",
        ),
        (
            "bad id",
            "login",
            "Your Slack user id is not one moor can act for, so nothing was changed.",
        ),
    ];
    for (n, (who, text, want)) in cases.into_iter().enumerate() {
        let kept = login(&slash(&format!("e{n}"), who, text), &db, 1).unwrap();
        assert_eq!(ack_text(&kept), want, "{text}");
        assert_eq!(kept.then, None);
    }
    let anyone = login(&slash("e9", "U777", "login github"), &db, 1).unwrap();
    assert!(anyone.then.is_some(), "no user list gates a login");
}

#[test]
fn logout_queues_one_unlink_of_the_named_account_and_acks_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (kept, job) = logout(&slash("e1", "U1", "logout github"), &db, 1).unwrap();
    assert_eq!(ack_text(&kept), "Unlinking your GitHub token…");
    let want = Job::Unlink {
        user: "U1".into(),
        account: Account::GitHub,
        response_url: Some("https://hooks.example/e1".into()),
    };
    assert_eq!(job, Some(want));
    let (_, job) = logout(&slash("e2", "U2", "logout"), &db, 1).unwrap();
    let Some(Job::Unlink { account, .. }) = job else {
        panic!("no unlink: {job:?}");
    };
    assert_eq!(account, Account::Claude);
    let (_, job) = logout(&slash("e1", "U1", "logout github"), &db, 2).unwrap();
    assert_eq!(job, None, "a redelivered command unlinks nothing twice");
    let (kept, job) = logout(&slash("e3", "U2", "logout codex"), &db, 1).unwrap();
    assert_eq!(
        ack_text(&kept),
        "moor cannot link that. It can link `claude` and `github`."
    );
    assert_eq!(job, None);
}

#[test]
fn a_submission_queues_one_check_and_keeps_only_its_envelope_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let token = "sk-ant-oat01-good-61d0e2";
    let sub = || submitted("e1", "U1", "claude", token);
    let (ack, job) = submission(sub(), &db, 1).unwrap();
    assert_eq!(modal_text(&ack), CHECKING);
    let want = TokenCheck {
        user: user("U1"),
        account: Account::Claude,
        view_id: "V1".into(),
        token: Secret::new(token.into()),
    };
    assert_eq!(job, Some(Job::CheckToken(want)));
    let (ack, job) = submission(sub(), &db, 2).unwrap();
    assert_eq!(modal_text(&ack), CHECKING);
    assert_eq!(job, None, "a redelivered submission checks nothing");
    drop(db);
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        let found = bytes.windows(token.len()).any(|w| w == token.as_bytes());
        assert!(!found, "the token reached slack.db");
    }
}

#[test]
fn a_submission_for_an_unknown_account_or_from_a_bad_id_checks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let cases = [
        (
            "e1",
            "U1",
            "codex",
            "This form names no account moor knows. Nothing was stored.",
        ),
        (
            "e2",
            "bad id",
            "claude",
            "Your Slack user id is not one moor can act for, so nothing was changed. \
             Nothing was stored.",
        ),
    ];
    for (id, who, account, want) in cases {
        let sub = submitted(id, who, account, "sk-ant-oat01-good-x");
        let (ack, job) = submission(sub, &db, 1).unwrap();
        assert_eq!(modal_text(&ack), want);
        assert_eq!(job, None);
    }
}

fn token_check(account: Account, token: &str) -> TokenCheck {
    TokenCheck {
        user: user("U1"),
        account,
        view_id: "V1".into(),
        token: Secret::new(token.into()),
    }
}

/// An agent's check that takes `accept` only and counts its calls.
struct FakeCheck {
    accept: &'static str,
    calls: Cell<usize>,
}

impl CredentialCheck for FakeCheck {
    fn check_format(&self, credential: &Secret) -> Result<(), String> {
        let ok = credential.expose().starts_with(b"fmt-");
        ok.then_some(()).ok_or_else(|| "bad shape".to_owned())
    }

    fn check(&self, _config_dir: &Path, credential: &Secret) -> Result<(), CheckError> {
        self.calls.set(self.calls.get() + 1);
        if credential.expose() == self.accept.as_bytes() {
            Ok(())
        } else {
            Err(CheckError::Refused("no".to_owned()))
        }
    }
}

#[test]
fn only_a_claude_token_the_agent_takes_passes() {
    let state = tempfile::tempdir().unwrap();
    let fake = FakeCheck {
        accept: "fmt-yes",
        calls: Cell::new(0),
    };
    let github = GitHub::new("http://127.0.0.1:9");
    let checks = |claude| Checks {
        claude,
        github: &github,
        state_dir: state.path(),
    };
    let run = |claude, token| checks(claude).run(&token_check(Account::Claude, token));
    assert_eq!(
        run(Ok(&fake), "nope"),
        Err("Claude refused this token: bad shape. Nothing was stored.".to_owned())
    );
    assert_eq!(fake.calls.get(), 0, "a bad shape asks the agent nothing");
    assert_eq!(
        run(Ok(&fake), "fmt-no"),
        Err("Claude refused this token: no. Nothing was stored.".to_owned())
    );
    assert_eq!(run(Ok(&fake), "fmt-yes"), Ok(()));
    assert_eq!(
        run(Err("no binary"), "fmt-yes"),
        Err("The coordinator is unavailable: no binary. Nothing was stored.".to_owned())
    );
    assert!(
        !state.path().join("credentials").exists(),
        "a check writes no credential file"
    );
}

#[test]
fn claude_checks_a_setup_token_in_the_users_own_config_dir() {
    let bin = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let claude = Claude::probe(&install(bin.path(), None), HostEnv::default()).unwrap();
    let github = GitHub::new("http://127.0.0.1:9");
    let checks = Checks {
        claude: Ok(&claude),
        github: &github,
        state_dir: state.path(),
    };
    let run = |token| checks.run(&token_check(Account::Claude, token));
    let want = "Claude refused this token: Failed to authenticate. API Error: 401 OAuth \
                access token is invalid. Nothing was stored.";
    assert_eq!(run("sk-ant-oat01-made-up"), Err(want.to_owned()));
    assert_eq!(run("sk-ant-oat01-good-1"), Ok(()));
    let env = std::fs::read_to_string(bin.path().join("run-1/env")).unwrap();
    let want = format!(
        "CLAUDE_CONFIG_DIR={}",
        state.path().join("claude/U1").display()
    );
    assert!(env.lines().any(|l| l == want), "{env}");
    let cwd = std::fs::read_to_string(bin.path().join("run-1/cwd")).unwrap();
    assert_eq!(cwd.trim_end(), "/");
    let want = "Claude refused this token: it is not a token that `claude setup-token` \
                prints. Nothing was stored.";
    assert_eq!(run(""), Err(want.to_owned()));
    assert!(
        !bin.path().join("run-2").exists(),
        "an empty token runs nothing"
    );
    assert!(
        !state.path().join("credentials").exists(),
        "a check writes no credential file"
    );
}

/// What the controller was asked, as `(user, command)`, and what it answers.
fn scripted<'a>(
    seen: &'a RefCell<Vec<(String, Request)>>,
    answer: impl Fn(&Request) -> Result<Response, ClientError> + 'a,
) -> impl FnMut(&str, Request) -> Result<Response, ClientError> + 'a {
    move |user, request| {
        let reply = answer(&request);
        seen.borrow_mut().push((user.to_owned(), request));
        reply
    }
}

#[test]
fn storing_enrolls_the_user_first_and_then_sets_the_named_secret() {
    let seen = RefCell::default();
    let mut call = scripted(&seen, |r| Ok(Response::ok(r.cmd(), Some(json!({})))));
    let token = Secret::new(b"gho_fake".to_vec());
    vault::store(&mut call, "U1", Account::GitHub, &token).unwrap();
    let seen = seen.take();
    let cmds: Vec<_> = seen.iter().map(|(u, r)| (u.as_str(), r.cmd())).collect();
    assert_eq!(cmds, [("U1", "enroll"), ("U1", "secret-set")]);
    let Request::SecretSet(set) = &seen[1].1 else {
        panic!("{:?}", seen[1]);
    };
    assert_eq!(set.name.as_str(), "github");
    assert_eq!(set.value.secret().expose(), b"gho_fake");
}

#[test]
fn a_failed_enroll_stops_before_the_secret_and_names_the_controllers_reason() {
    let seen = RefCell::default();
    let refuse = |r: &Request| {
        let error = moor_core::response::ErrorView {
            kind: moor_core::response::ErrorKind::Refused,
            message: "the vault is sealed".to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
        };
        Ok(Response::err(r.cmd(), error))
    };
    let mut call = scripted(&seen, refuse);
    let token = Secret::new(b"gho_fake".to_vec());
    let why = vault::store(&mut call, "U1", Account::Claude, &token).unwrap_err();
    assert_eq!(why, "the vault is sealed");
    assert_eq!(
        seen.borrow().len(),
        1,
        "no secret-set after a failed enroll"
    );
}

#[test]
fn deleting_names_the_account_secret_and_says_whether_one_was_there() {
    for (changed, want) in [(true, true), (false, false)] {
        let seen = RefCell::default();
        let mut call = scripted(&seen, |r| {
            Ok(Response::ok(
                r.cmd(),
                Some(json!({"name": "github", "changed": changed})),
            ))
        });
        assert_eq!(vault::delete(&mut call, "U2", Account::GitHub), Ok(want));
        let seen = seen.take();
        let Request::SecretDelete(delete) = &seen[0].1 else {
            panic!("{:?}", seen[0]);
        };
        assert_eq!((seen[0].0.as_str(), delete.name.as_str()), ("U2", "github"));
        assert_eq!(seen.len(), 1);
    }
}
