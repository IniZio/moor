//! Tests for Slack event dedup and thread binding.

use moor_core::seam::clock::Clock;
use moor_core::BoxName;

use crate::store::tests::{new_task, open};
use crate::store::{SlackPrompt, SlackThread, Store};

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

#[test]
fn event_is_new_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert!(store.slack_event_once("Ev1").unwrap());
    assert!(!store.slack_event_once("Ev1").unwrap());
    assert!(store.slack_event_once("Ev2").unwrap());
}

#[test]
fn thread_binding_first_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "first");
    new_task(&store, "second");
    assert_eq!(store.slack_thread("C1", "1.1").unwrap(), None);
    assert!(store
        .bind_slack_thread("C1", "1.1", &name("first"), "U1")
        .unwrap());
    assert_eq!(
        store.slack_thread("C1", "1.1").unwrap(),
        Some(name("first"))
    );
    assert!(!store
        .bind_slack_thread("C1", "1.1", &name("second"), "U2")
        .unwrap());
    assert_eq!(
        store.slack_thread("C1", "1.1").unwrap(),
        Some(name("first"))
    );
    assert_eq!(store.slack_thread("C1", "2.2").unwrap(), None);
}

#[test]
fn existing_v1_db_gains_tables() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    new_task(&store, "t");
    store
        .writer()
        .unwrap()
        .execute_batch("DROP TABLE slack_events; DROP TABLE bindings;")
        .unwrap();
    drop(store);
    let path = dir.path().join("state").join("moor.db");
    let store = Store::open(&path, clock).unwrap();
    assert!(store.slack_event_once("Ev1").unwrap());
    assert!(store
        .bind_slack_thread("C1", "1.1", &name("t"), "U1")
        .unwrap());
}

#[test]
fn owner_kept_from_first_bind() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "first");
    new_task(&store, "second");
    assert_eq!(store.slack_thread_owner("C1", "1.1").unwrap(), None);
    store
        .bind_slack_thread("C1", "1.1", &name("first"), "U1")
        .unwrap();
    store
        .bind_slack_thread("C1", "1.1", &name("second"), "U2")
        .unwrap();
    assert_eq!(
        store.slack_thread_owner("C1", "1.1").unwrap().as_deref(),
        Some("U1")
    );
}

#[test]
fn status_ts_set_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    store
        .bind_slack_thread("C1", "1.1", &name("t"), "U1")
        .unwrap();
    assert_eq!(
        store.slack_threads().unwrap().first().unwrap().status_ts,
        None
    );
    store.set_slack_status_ts("C1", "1.1", "9.9").unwrap();
    assert_eq!(
        store
            .slack_threads()
            .unwrap()
            .first()
            .unwrap()
            .status_ts
            .as_deref(),
        Some("9.9")
    );
}

#[test]
fn threads_listed_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    new_task(&store, "a");
    new_task(&store, "b");
    assert!(store.slack_threads().unwrap().is_empty());
    store
        .bind_slack_thread("C1", "2.2", &name("b"), "U2")
        .unwrap();
    clock.sleep(std::time::Duration::from_secs(1));
    store
        .bind_slack_thread("C1", "1.1", &name("a"), "U1")
        .unwrap();
    let threads = store.slack_threads().unwrap();
    assert_eq!(
        threads,
        vec![
            SlackThread {
                channel: "C1".into(),
                thread_ts: "2.2".into(),
                task: name("b"),
                owner: "U2".into(),
                status_ts: None,
            },
            SlackThread {
                channel: "C1".into(),
                thread_ts: "1.1".into(),
                task: name("a"),
                owner: "U1".into(),
                status_ts: None,
            },
        ]
    );
}

fn prompt(event_id: i64, thread_ts: &str, message_ts: &str) -> SlackPrompt {
    SlackPrompt {
        event_id,
        channel: "C1".into(),
        thread_ts: thread_ts.into(),
        message_ts: message_ts.into(),
        task: name("t"),
        question: "which?".into(),
        options: vec!["a".into(), "b \"quoted\"".into()],
        answer: None,
    }
}

#[test]
fn prompt_posted_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    assert!(!store.slack_prompt_posted(7).unwrap());
    store.add_slack_prompt(&prompt(7, "1.1", "2.2")).unwrap();
    assert!(store.slack_prompt_posted(7).unwrap());
    assert!(!store.slack_prompt_posted(8).unwrap());
}

#[test]
fn add_prompt_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    store.add_slack_prompt(&prompt(7, "1.1", "2.2")).unwrap();
    store.add_slack_prompt(&prompt(7, "1.1", "9.9")).unwrap();
    assert_eq!(
        store.slack_prompt_by_message("C1", "2.2").unwrap(),
        Some(prompt(7, "1.1", "2.2"))
    );
    assert_eq!(store.slack_prompt_by_message("C1", "9.9").unwrap(), None);
}

#[test]
fn prompt_found_by_message() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    store.add_slack_prompt(&prompt(7, "1.1", "2.2")).unwrap();
    assert_eq!(
        store.slack_prompt_by_message("C1", "2.2").unwrap(),
        Some(prompt(7, "1.1", "2.2"))
    );
    assert_eq!(store.slack_prompt_by_message("C2", "2.2").unwrap(), None);
    assert_eq!(store.slack_prompt_by_message("C1", "3.3").unwrap(), None);
}

#[test]
fn open_prompt_is_newest_unanswered() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    assert_eq!(store.open_slack_prompt("C1", "1.1").unwrap(), None);
    store.add_slack_prompt(&prompt(5, "1.1", "2.2")).unwrap();
    store.add_slack_prompt(&prompt(9, "1.1", "3.3")).unwrap();
    store.add_slack_prompt(&prompt(12, "4.4", "5.5")).unwrap();
    assert_eq!(
        store
            .open_slack_prompt("C1", "1.1")
            .unwrap()
            .unwrap()
            .event_id,
        9
    );
    assert!(store.answer_slack_prompt(9, "1. a").unwrap());
    assert_eq!(
        store
            .open_slack_prompt("C1", "1.1")
            .unwrap()
            .unwrap()
            .event_id,
        5
    );
    assert!(store.answer_slack_prompt(5, "2. b").unwrap());
    assert_eq!(store.open_slack_prompt("C1", "1.1").unwrap(), None);
}

#[test]
fn prompt_answered_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    store.add_slack_prompt(&prompt(7, "1.1", "2.2")).unwrap();
    assert!(store.answer_slack_prompt(7, "1. a").unwrap());
    assert!(!store.answer_slack_prompt(7, "2. b").unwrap());
    assert!(!store.answer_slack_prompt(99, "1. a").unwrap());
    let got = store.slack_prompt_by_message("C1", "2.2").unwrap().unwrap();
    assert_eq!(got.answer.as_deref(), Some("1. a"));
}

fn primary_session(store: &Store, id: moor_core::BoxId) -> i64 {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT id FROM sessions WHERE box = ?1 AND ordinal = 1",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn binding_row_keys_front_and_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    store
        .bind_slack_thread("C1", "1.1", &name("t"), "U1")
        .unwrap();
    let row: (String, String, i64) = store
        .reader()
        .unwrap()
        .query_row("SELECT front, conv_key, session FROM bindings", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(
        row,
        ("slack".into(), "C1/1.1".into(), primary_session(&store, id))
    );
}

#[test]
fn binding_to_unknown_box_fails() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    assert!(store
        .bind_slack_thread("C1", "1.1", &name("ghost"), "U1")
        .is_err());
    assert_eq!(store.slack_thread("C1", "1.1").unwrap(), None);
}

#[test]
fn prompt_row_keys_session() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = new_task(&store, "t");
    store.add_slack_prompt(&prompt(7, "1.1", "2.2")).unwrap();
    let session: i64 = store
        .reader()
        .unwrap()
        .query_row("SELECT session FROM slack_prompts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(session, primary_session(&store, id));
}

fn bind_host_thread(store: &Store, thread_ts: &str) {
    let session = store.open_host_session("/w", None, None).unwrap();
    assert!(store
        .bind_session("slack", &format!("C1/{thread_ts}"), session, "U1")
        .unwrap());
}

#[test]
fn host_bound_thread_has_no_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    bind_host_thread(&store, "1.0");
    assert_eq!(store.slack_thread("C1", "1.0").unwrap(), None);
}

#[test]
fn threads_skip_host_bound_and_keep_box_bound() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    new_task(&store, "t");
    bind_host_thread(&store, "1.0");
    store
        .bind_slack_thread("C1", "2.0", &name("t"), "U2")
        .unwrap();
    let threads = store.slack_threads().unwrap();
    let seen: Vec<_> = threads
        .iter()
        .map(|t| (t.thread_ts.as_str(), &t.task))
        .collect();
    assert_eq!(seen, [("2.0", &name("t"))]);
}
