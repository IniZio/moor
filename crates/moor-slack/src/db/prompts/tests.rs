//! Tests for the prompt rows: one per Box and event, found by message, open until answered
//! once or closed.
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
            prompt: "fix".into(),
            base: None,
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
        message_ts: Some(&format!("m{event_id}")),
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
        (found.channel.as_str(), found.message_ts.as_deref()),
        ("C1", Some("m5"))
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

#[test]
fn a_prompt_is_found_by_its_message_in_its_channel_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1"]);
    post(&db, "slack-1-1", 5);
    let found = db.prompt_by_message("C1", "m5").unwrap().unwrap();
    assert_eq!(found.event_id, 5);
    assert_eq!(db.prompt(found.id).unwrap(), Some(found));
    assert!(db.prompt_by_message("C2", "m5").unwrap().is_none());
}

#[test]
fn a_prompt_is_answered_once_and_a_closed_one_not_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1"]);
    post(&db, "slack-1-1", 5);
    post(&db, "slack-1-1", 6);
    let id = |m: &str| db.prompt_by_message("C1", m).unwrap().unwrap().id;
    assert!(db.answer_prompt(id("m5"), "2. b", "U1").unwrap());
    assert!(!db.answer_prompt(id("m5"), "1. a", "U1").unwrap());
    db.close_prompt(id("m6"), "working").unwrap();
    assert!(!db.answer_prompt(id("m6"), "1. a", "U1").unwrap());
    let state = |m: &str| db.prompt_by_message("C1", m).unwrap().unwrap().state;
    let answered = PromptState::Answered {
        label: "2. b".into(),
        by: "U1".into(),
    };
    assert_eq!(state("m5"), answered);
    assert_eq!(state("m6"), PromptState::Closed("working".into()));
    assert!(db.open_prompts("C1", "1.0").unwrap().is_empty());
}

#[test]
fn an_answer_is_shown_once_its_box_took_it_until_its_message_is_edited() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1"]);
    let name = BoxName::parse_allow_bare("slack-1-1").unwrap();
    post(&db, "slack-1-1", 5);
    let id = db.prompt_by_message("C1", "m5").unwrap().unwrap().id;
    db.answer_prompt(id, "1. a", "U1").unwrap();
    assert!(
        db.unshown_answers(&name).unwrap().is_empty(),
        "not sent yet"
    );
    db.answer_sent(id).unwrap();
    assert_eq!(db.unshown_answers(&name).unwrap().len(), 1);
    db.mark_answer_shown(id, 2).unwrap();
    assert!(db.unshown_answers(&name).unwrap().is_empty());
}

#[test]
fn open_prompts_cover_every_box_of_the_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1", "slack-1-2"]);
    post(&db, "slack-1-1", 5);
    post(&db, "slack-1-2", 6);
    let open = db.open_prompts("C1", "1.0").unwrap();
    let names: Vec<_> = open.iter().map(|p| p.box_name.as_str()).collect();
    assert_eq!(names, ["slack-1-1", "slack-1-2"]);
}

#[test]
fn a_recorded_answer_survives_a_close_and_reopens_only_while_unsent() {
    let dir = tempfile::tempdir().unwrap();
    let db = with_box(dir.path(), &["slack-1-1"]);
    post(&db, "slack-1-1", 5);
    post(&db, "slack-1-1", 6);
    let id = |m: &str| db.prompt_by_message("C1", m).unwrap().unwrap().id;
    let state = |m: &str| db.prompt_by_message("C1", m).unwrap().unwrap().state;
    db.answer_prompt(id("m5"), "1. a", "U1").unwrap();
    db.close_prompt(id("m5"), "working").unwrap();
    assert!(matches!(state("m5"), PromptState::Answered { .. }));
    db.reopen_prompt(id("m5")).unwrap();
    assert_eq!(state("m5"), PromptState::Open);
    db.answer_prompt(id("m6"), "2. b", "U1").unwrap();
    db.answer_sent(id("m6")).unwrap();
    db.reopen_prompt(id("m6")).unwrap();
    assert!(matches!(state("m6"), PromptState::Answered { .. }));
}
