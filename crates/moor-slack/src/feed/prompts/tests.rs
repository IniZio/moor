//! Tests for the questions the feed folds and the prompts it closes.
#![allow(clippy::indexing_slicing, reason = "test code")]

use std::sync::Arc;

use moor_core::name::{BoxId, BoxName};
use moor_core::secret::Secret;
use serde_json::json;

use super::*;
use crate::api::Api;
use crate::db::SlackThread;
use crate::identity::{Link, Team};
use moor_client::Client;

/// The id the tests give their Box.
const ID: BoxId = BoxId::new(7);

/// A feed whose Slack cannot be reached, so any post it tries fails.
fn feed(dir: &std::path::Path) -> Feed {
    Feed {
        link: Link::new(Client::new(dir.join("c.sock")), Team::parse("T1").unwrap()),
        db_path: dir.join("slack.db"),
        api: Arc::new(Api::new("http://127.0.0.1:9").unwrap()),
        bot_token: Arc::new(Secret::new(b"xoxb-test".to_vec())),
        log: Arc::new(|_| {}),
        update_every: super::super::status::UPDATE_EVERY,
        clock: || 9,
    }
}

/// Records a thread with Box `slack-1-1` bound to [`ID`] and returns the Box row.
fn bound_box(db: &Db) -> SlackBox {
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    let mut row = SlackBox {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        box_name: BoxName::parse_allow_bare("slack-1-1").unwrap(),
        repo: "acme/app".into(),
        goal: "fix it".into(),
        requested_by: "U1".into(),
        parent: None,
        state: BoxState::Starting,
    };
    db.insert_starting(&row, 1).unwrap();
    db.bind(&row.box_name, ID, None).unwrap();
    row.state = BoxState::Bound {
        box_id: ID,
        branch: None,
    };
    row
}

/// Records a prompt of `row` for event `event_id` and returns it.
fn prompt_of(db: &Db, row: &SlackBox, event_id: i64) -> Prompt {
    let posted = Posted {
        box_name: &row.box_name,
        event_id,
        message_ts: Some(&format!("9.{event_id}")),
        question: "Which db?",
        options: &["pg".to_owned(), "sqlite".to_owned()],
    };
    db.insert_prompt(&posted, 1).unwrap();
    let open = db.open_box_prompts(&row.box_name).unwrap();
    open.into_iter().find(|p| p.event_id == event_id).unwrap()
}

/// A fold in which Box [`ID`] got `events`, each `(cursor, kind)`, with question `q<cursor>`.
fn folded(events: &[(i64, &str)]) -> Fold {
    let mut fold = Fold::default();
    for (cursor, kind) in events {
        let payload = json!({"question": format!("q{cursor}"), "options": ["a"]});
        super::fold(&mut fold, ID, kind, *cursor, &payload);
        fold.views.entry(ID).or_default().fold(kind, &payload);
    }
    fold
}

#[test]
fn fold_opens_replaces_and_clears() {
    let mut fold = Fold::default();
    let asked = json!({"question": "q1", "options": ["a"]});
    super::fold(&mut fold, ID, "needs_input", 1, &asked);
    assert_eq!(fold.asked[&ID].cursor, 1);
    assert_eq!(fold.asked[&ID].options, vec!["a".to_owned()]);
    let bare = json!({"question": null, "options": []});
    super::fold(&mut fold, ID, "needs_input", 2, &bare);
    assert_eq!(fold.asked[&ID].cursor, 2);
    assert_eq!(fold.asked[&ID].question, "");
    super::fold(&mut fold, ID, "box_ready", 3, &json!({}));
    assert!(fold.asked.contains_key(&ID));
    super::fold(&mut fold, ID, "working", 4, &json!({}));
    assert!(fold.asked.is_empty());
}

#[test]
fn a_prompt_stays_open_only_while_its_box_waits_on_that_question() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("slack.db")).unwrap();
    let row = bound_box(&db);
    let prompt = prompt_of(&db, &row, 5);
    let reason = |events: &[(i64, &str)], state: &BoxState| {
        let fold = folded(events);
        moved_on(&prompt, state, fold.views.get(&ID), fold.asked.get(&ID))
    };
    let bound = row.state;
    assert_eq!(reason(&[(5, "needs_input")], &bound), None);
    let newer = [(5, "needs_input"), (6, "needs_input")];
    assert_eq!(reason(&newer, &bound), Some("waiting on a newer question"));
    let idle = [(5, "needs_input"), (6, "idled")];
    assert_eq!(reason(&idle, &bound), Some("idle"));
    let working = [(5, "needs_input"), (6, "working")];
    assert_eq!(reason(&working, &bound), Some("working"));
    let gone = BoxState::Gone {
        box_id: ID,
        branch: None,
    };
    assert_eq!(reason(&[(5, "needs_input")], &gone), Some("closed"));
}

#[test]
fn the_feed_closes_a_prompt_whose_box_moved_on_and_keeps_the_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let row = bound_box(&db);
    let old = prompt_of(&db, &row, 5);
    let live = prompt_of(&db, &row, 6);
    let fold = folded(&[(5, "needs_input"), (6, "needs_input")]);
    feed.close_moved_on(&db, &fold, &row);
    let open: Vec<i64> = db
        .open_box_prompts(&row.box_name)
        .unwrap()
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(open, [live.id], "prompt {} is closed", old.id);
}

#[test]
fn a_prompt_that_cannot_be_posted_is_not_recorded_and_is_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let row = bound_box(&db);
    let fold = folded(&[(5, "needs_input")]);
    assert!(!feed.prompts(&db, &fold, &row));
    assert!(!db.has_prompt(&row.box_name, 5).unwrap());
}

#[test]
fn a_box_that_is_closing_gets_no_new_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let mut row = bound_box(&db);
    db.mark_closing(&row.box_name).unwrap();
    row.state = BoxState::Closing {
        box_id: ID,
        branch: None,
    };
    let fold = folded(&[(5, "needs_input")]);
    assert!(feed.prompts(&db, &fold, &row), "nothing to post");
    assert!(!db.has_prompt(&row.box_name, 5).unwrap());
}
