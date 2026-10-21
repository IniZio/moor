//! Tests for the prompt rows: one per Box and event, open until closed.
#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::name::{BoxId, BoxName};

use super::super::{BoxState, Db, SlackBox, SlackThread};
use super::{Posted, PromptState};

/// A database with thread `1.0` of `C1` and Box `name` bound in it, asked for by `U1`.
fn with_box(dir: &std::path::Path, names: &[&str]) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    for (i, name) in names.iter().enumerate() {
        let boxed = SlackBox {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            box_name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "acme/app".into(),
            goal: "fix".into(),
            requested_by: "U1".into(),
            parent: None,
            state: BoxState::Starting,
        };
        db.insert_starting(&boxed, 1).unwrap();
        let id = BoxId::new(i64::try_from(i).unwrap() + 1);
        db.bind(&boxed.box_name, id, None).unwrap();
    }
    db
}

/// Records a prompt of Box `name` for event `event_id`, with message ts `m<event_id>`.
fn post(db: &Db, name: &str, event_id: i64) {
    let box_name = BoxName::parse_allow_bare(name).unwrap();
    let posted = Posted {
        box_name: &box_name,
        event_id,
        message_ts: &format!("m{event_id}"),
        question: "Which?",
        options: &["a".to_owned(), "b".to_owned()],
    };
    db.insert_prompt(&posted, 1).unwrap();
}

#[test]
fn a_prompt_is_recorded_once_per_box_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1"]);
    let name = BoxName::parse_allow_bare("slack-1-1").unwrap();
    assert!(!db.has_prompt(&name, 5).unwrap());
    post(&db, "slack-1-1", 5);
    post(&db, "slack-1-1", 5);
    assert!(db.has_prompt(&name, 5).unwrap());
    let open = db.open_box_prompts(&name).unwrap();
    assert_eq!(open.len(), 1);
    let found = &open[0];
    assert_eq!(
        (found.channel.as_str(), found.message_ts.as_str()),
        ("C1", "m5")
    );
    assert_eq!(found.requested_by, "U1");
    assert_eq!(found.options, vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(found.state, PromptState::Open);
}

#[test]
fn a_closed_prompt_is_no_longer_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1", "slack-1-2"]);
    post(&db, "slack-1-1", 5);
    post(&db, "slack-1-1", 6);
    post(&db, "slack-1-2", 7);
    let one = BoxName::parse_allow_bare("slack-1-1").unwrap();
    let first = db.open_box_prompts(&one).unwrap()[0].id;
    db.close_prompt(first, "working").unwrap();
    let open: Vec<i64> = db
        .open_box_prompts(&one)
        .unwrap()
        .iter()
        .map(|p| p.event_id)
        .collect();
    assert_eq!(open, [6], "only the other prompt of this Box stays open");
    assert!(db.unshown_answers(&one).unwrap().is_empty());
}
