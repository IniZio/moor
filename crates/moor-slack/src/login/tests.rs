//! Tests for `/moor login`, `/moor logout` and what the worker asks of the controller.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code reads JSON by key and fails on a wrong request"
)]

use std::cell::RefCell;
use std::path::Path;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{login, logout, vault};
use crate::account::Account;
use crate::db::Db;
use crate::socket::{Kept, Slash};
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

fn ack_text(kept: &Kept) -> &str {
    kept.ack.as_ref().unwrap()["text"].as_str().unwrap()
}

#[test]
fn login_answers_with_the_account_page_link_and_opens_no_modal() {
    let kept = login(Some("https://moor.example.com"));
    assert!(kept.then.is_none(), "no modal opens after the ack");
    let text = ack_text(&kept);
    assert!(text.contains("https://moor.example.com/"), "{text}");
    assert!(!text.contains("//moor.example.com//"), "{text}");
}

#[test]
fn login_without_an_account_url_tells_the_user_to_ask_the_operator() {
    let kept = login(None);
    assert!(kept.then.is_none());
    assert!(
        ack_text(&kept).contains("account_url"),
        "{}",
        ack_text(&kept)
    );
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
        "moor cannot unlink that. It can unlink `claude` and `github`."
    );
    assert_eq!(job, None);
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
