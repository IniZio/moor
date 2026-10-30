//! Tests for `/moor profile`: what it parses and queues.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use moor_core::name::ProfileSource;

use super::{parse, slash, Change, ProfileJob};
use crate::db::Db;
use crate::socket::Slash;
use crate::worker::Job;

fn command(envelope_id: &str, text: &str) -> Slash {
    Slash {
        envelope_id: envelope_id.into(),
        channel: "C1".into(),
        user: "U0FIRST".into(),
        text: text.into(),
        trigger_id: None,
        response_url: Some("https://hooks.example.com/r".into()),
    }
}

fn ack_text(kept: &crate::socket::Kept) -> String {
    kept.ack.as_ref().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn parses_the_three_forms() {
    let want = ProfileSource::parse("https://github.com/acme/dots#main").unwrap();
    assert_eq!(parse(&[]), Ok(Change::Show));
    assert_eq!(parse(&["remove"]), Ok(Change::Remove));
    assert_eq!(
        parse(&["set", "https://github.com/acme/dots#main"]),
        Ok(Change::Set(want))
    );
}

#[test]
fn bad_forms_get_the_usage_or_the_url_refusal() {
    for bad in [
        &["set"][..],
        &["remove", "x"],
        &["frob"],
        &["set", "a", "b"],
    ] {
        let text = parse(bad).unwrap_err();
        assert!(
            text.starts_with("Usage: `/moor profile`"),
            "{bad:?}: {text}"
        );
    }
    let text = parse(&["set", "https://gitlab.com/acme/dots"]).unwrap_err();
    assert!(
        text.starts_with("a profile must be https://github.com/OWNER/NAME"),
        "{text}"
    );
}

#[test]
fn a_command_queues_a_job_for_that_user_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("slack.db")).unwrap();
    let s = command("e1", "profile set https://github.com/acme/dots");
    let (kept, job) = slash(&s, &db, 1).unwrap();
    assert_eq!(ack_text(&kept), "Setting your profile…");
    let Some(Job::Profile(ProfileJob {
        user,
        change,
        response_url,
    })) = job
    else {
        panic!("no job");
    };
    assert_eq!(user, "U0FIRST");
    assert!(matches!(change, Change::Set(_)));
    assert!(response_url.is_some());
    let (_, again) = slash(&s, &db, 2).unwrap();
    assert_eq!(again, None);
}
