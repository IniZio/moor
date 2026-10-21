//! Tests for `/moor login`, `/moor logout`, the submitted modal and the credential check.
#![allow(clippy::indexing_slicing, reason = "test code reads JSON by key")]

use std::cell::Cell;
use std::path::Path;

use moor_core::secret::Secret;
use serde_json::Value;

use super::{check, form, login, logout, submission, TokenCheck, CHECKING, LINKED};
use crate::coordinator::claude::tests::install;
use crate::coordinator::claude::{Claude, HostEnv};
use crate::credentials::{
    credential_path, store, AgentKind, CheckError, CredentialCheck, SlackUser,
};
use crate::db::Db;
use crate::socket::{AfterAck, Kept, Slash, TokenSubmission};
use crate::worker::Job;

const CLAUDE: AgentKind = AgentKind::Claude;

fn users() -> Vec<String> {
    vec!["U1".to_owned(), "bad id".to_owned()]
}

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
    }
}

fn submitted(envelope_id: &str, user: &str, agent: &str, token: &str) -> TokenSubmission {
    TokenSubmission {
        envelope_id: envelope_id.into(),
        user: user.into(),
        view_id: "V1".into(),
        agent: agent.into(),
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
fn login_opens_the_modal_for_the_default_agent_once_after_an_empty_ack() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    for (n, text) in ["login", "login claude"].into_iter().enumerate() {
        let id = format!("e{n}");
        let kept = login(&slash(&id, "U1", text), &db, &users(), 1).unwrap();
        assert_eq!(kept.ack, None);
        let open = AfterAck::OpenLogin {
            trigger_id: format!("trig-{id}"),
            agent: CLAUDE,
        };
        assert_eq!(kept.then, Some(open), "{text}");
    }
    let again = login(&slash("e0", "U1", "login"), &db, &users(), 2).unwrap();
    assert_eq!(
        again.then, None,
        "a redelivered command opens no second modal"
    );
    let modal = form(CLAUDE);
    assert_eq!(modal["private_metadata"], "claude");
    let prompt = "Paste the token that `claude setup-token` printed on your machine.";
    assert_eq!(modal["blocks"][0]["label"]["text"], prompt);
}

#[test]
fn login_refuses_an_unknown_agent_a_user_off_the_list_or_a_bad_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let cases = [
        (
            "U1",
            "login codex",
            "moor knows no agent called that. It knows `claude`.",
        ),
        (
            "U1",
            "login claude extra",
            "Usage: `/moor login [agent]` or `/moor logout [agent]`.",
        ),
        ("U9", "login", "<@U9> is not allowed to link a token here."),
        (
            "bad id",
            "login",
            "Your Slack user id cannot name a token file, so nothing was changed.",
        ),
    ];
    for (n, (who, text, want)) in cases.into_iter().enumerate() {
        let kept = login(&slash(&format!("e{n}"), who, text), &db, &users(), 1).unwrap();
        assert_eq!(ack_text(&kept), want, "{text}");
        assert_eq!(kept.then, None);
    }
}

#[test]
fn logout_deletes_only_the_users_credential_for_the_agent() {
    let state = tempfile::tempdir().unwrap();
    for id in ["U1", "U2"] {
        store(state.path(), CLAUDE, &user(id), &Secret::new(b"t".to_vec())).unwrap();
    }
    let kept = logout(&slash("e1", "U1", "logout"), state.path(), &|_| {});
    assert_eq!(ack_text(&kept), "Unlinked.");
    assert!(!credential_path(state.path(), CLAUDE, &user("U1")).exists());
    assert!(credential_path(state.path(), CLAUDE, &user("U2")).exists());
    let kept = logout(&slash("e2", "U1", "logout claude"), state.path(), &|_| {});
    assert_eq!(ack_text(&kept), "Unlinked.");
    let kept = logout(&slash("e3", "U2", "logout codex"), state.path(), &|_| {});
    assert_eq!(
        ack_text(&kept),
        "moor knows no agent called that. It knows `claude`."
    );
    assert!(credential_path(state.path(), CLAUDE, &user("U2")).exists());
}

#[test]
fn a_logout_that_cannot_delete_the_file_says_so_and_logs_why() {
    let state = tempfile::tempdir().unwrap();
    let path = credential_path(state.path(), CLAUDE, &user("U1"));
    std::fs::create_dir_all(path.join("blocker")).unwrap();
    let logged = std::cell::RefCell::new(Vec::new());
    let log = |line: &str| logged.borrow_mut().push(line.to_owned());
    let kept = logout(&slash("e1", "U1", "logout"), state.path(), &log);
    assert_eq!(
        ack_text(&kept),
        "Could not delete your token file. The operator's log says why."
    );
    let logged = logged.into_inner();
    assert_eq!(logged.len(), 1);
    assert!(logged[0].starts_with("logout of U1: "), "{logged:?}");
}

#[test]
fn a_submission_queues_one_check_and_keeps_only_its_envelope_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let token = "sk-ant-oat01-good-61d0e2";
    let sub = || submitted("e1", "U1", "claude", token);
    let (ack, job) = submission(sub(), &db, &users(), 1).unwrap();
    assert_eq!(modal_text(&ack), CHECKING);
    let want = TokenCheck {
        user: user("U1"),
        agent: CLAUDE,
        view_id: "V1".into(),
        token: Secret::new(token.into()),
    };
    assert_eq!(job, Some(Job::CheckToken(want)));
    let (ack, job) = submission(sub(), &db, &users(), 2).unwrap();
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
fn a_submission_from_a_user_off_the_list_or_for_an_unknown_agent_checks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let cases = [
        (
            "e1",
            "U9",
            "claude",
            "You are not allowed to link a token here. Nothing was stored.",
        ),
        (
            "e2",
            "U1",
            "codex",
            "This form names no agent moor knows. Nothing was stored.",
        ),
    ];
    for (id, who, agent, want) in cases {
        let sub = submitted(id, who, agent, "sk-ant-oat01-good-x");
        let (ack, job) = submission(sub, &db, &users(), 1).unwrap();
        assert_eq!(modal_text(&ack), want);
        assert_eq!(job, None);
    }
}

/// A check for `U1` of `token` for Claude.
fn token_check(token: &str) -> TokenCheck {
    TokenCheck {
        user: user("U1"),
        agent: CLAUDE,
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
fn only_a_credential_the_agent_takes_is_stored() {
    let state = tempfile::tempdir().unwrap();
    let fake = FakeCheck {
        accept: "fmt-yes",
        calls: Cell::new(0),
    };
    let path = credential_path(state.path(), CLAUDE, &user("U1"));
    let text = check(&token_check("nope"), Ok(&fake), state.path());
    assert_eq!(
        text,
        "Claude refused this token: bad shape. Nothing was stored."
    );
    assert_eq!(fake.calls.get(), 0, "a bad shape asks the agent nothing");
    let text = check(&token_check("fmt-no"), Ok(&fake), state.path());
    assert_eq!(text, "Claude refused this token: no. Nothing was stored.");
    assert!(!path.exists());
    assert_eq!(
        check(&token_check("fmt-yes"), Ok(&fake), state.path()),
        LINKED
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"fmt-yes");
    let text = check(&token_check("fmt-yes"), Err("no binary"), state.path());
    assert_eq!(
        text,
        "The coordinator is unavailable: no binary. Nothing was stored."
    );
}

#[test]
fn a_token_the_agent_takes_but_the_file_cannot_hold_is_reported_not_linked() {
    let state = tempfile::tempdir().unwrap();
    let parent = state.path().join("credentials");
    std::fs::write(&parent, "a file, not a dir").unwrap();
    let fake = FakeCheck {
        accept: "fmt-yes",
        calls: Cell::new(0),
    };
    let text = check(&token_check("fmt-yes"), Ok(&fake), state.path());
    let want = format!(
        "Could not store your token: {}: not a directory that this user owns. \
         Nothing was stored.",
        parent.display()
    );
    assert_eq!(text, want);
    assert_eq!(fake.calls.get(), 1);
}

#[test]
fn claude_checks_a_setup_token_in_the_users_own_config_dir() {
    let bin = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let claude = Claude::probe(&install(bin.path(), None), HostEnv::default()).unwrap();
    let text = check(
        &token_check("sk-ant-oat01-made-up"),
        Ok(&claude),
        state.path(),
    );
    let want = "Claude refused this token: Failed to authenticate. API Error: 401 OAuth \
                access token is invalid. Nothing was stored.";
    assert_eq!(text, want);
    let token = "sk-ant-oat01-good-1";
    assert_eq!(
        check(&token_check(token), Ok(&claude), state.path()),
        LINKED
    );
    let path = credential_path(state.path(), CLAUDE, &user("U1"));
    assert_eq!(std::fs::read(path).unwrap(), token.as_bytes());
    let env = std::fs::read_to_string(bin.path().join("run-1/env")).unwrap();
    let want = format!(
        "CLAUDE_CONFIG_DIR={}",
        state.path().join("claude/U1").display()
    );
    assert!(env.lines().any(|l| l == want), "{env}");
    let cwd = std::fs::read_to_string(bin.path().join("run-1/cwd")).unwrap();
    assert_eq!(cwd.trim_end(), "/");
    let text = check(&token_check(""), Ok(&claude), state.path());
    let want = "Claude refused this token: it is not a token that `claude setup-token` \
                prints. Nothing was stored.";
    assert_eq!(text, want);
    assert!(
        !bin.path().join("run-2").exists(),
        "an empty token runs nothing"
    );
}
