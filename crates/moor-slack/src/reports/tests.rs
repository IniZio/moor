//! Tests for the reporter against a real `slack.db`, a scripted controller and a recording
//! Slack.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::cell::RefCell;
use std::path::Path;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::{json, Value};

use super::{poll, Fx, GIVE_UP};
use crate::db::{BoxState, Db, SlackThread};

/// A database with threads `1.0` and `2.0` of channel `C1`, both started by `U1`.
fn open(dir: &Path) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    for ts in ["1.0", "2.0"] {
        let thread = SlackThread {
            channel: "C1".into(),
            thread_ts: ts.into(),
            owner: "U1".into(),
        };
        if db.thread("C1", ts).unwrap().is_none() {
            db.insert_thread(&thread, 1).unwrap();
        }
    }
    db
}

/// A run change of workflow `sec` from `front`, keyed to `key`.
fn run(seq: i64, (front, key): (&str, Option<&str>), outcome: &str, boxed: Option<&str>) -> Value {
    let mut v = json!({
        "kind": "run", "seq": seq, "workflow": "sec", "repo": "acme/api",
        "due_at": 0, "outcome": outcome,
        "origin": {"front": front, "key": key},
    });
    if let Some(b) = boxed {
        v["box"] = json!(b);
    }
    if outcome == "refused" {
        v["detail"] = json!("repo not found");
    }
    v
}

fn ours(seq: i64, outcome: &str, boxed: Option<&str>) -> Value {
    run(seq, ("slack", Some("C1/1.0")), outcome, boxed)
}

/// The controller's stream and what Slack was sent.
struct World {
    changes: Vec<Value>,
    asked: RefCell<Vec<Option<i64>>>,
    posts: RefCell<Vec<(String, String, String)>>,
    /// Threads whose posts fail.
    broken: Vec<&'static str>,
}

impl World {
    fn new(changes: Vec<Value>) -> Self {
        Self {
            changes,
            asked: RefCell::new(Vec::new()),
            posts: RefCell::new(Vec::new()),
            broken: Vec::new(),
        }
    }

    fn poll(&self, db: &Db) -> Result<(), String> {
        let mut call = |request: Request| match request {
            Request::WorkflowRuns(r) => {
                self.asked.borrow_mut().push(r.after);
                let after = r.after.unwrap_or(0);
                let new: Vec<_> = self
                    .changes
                    .iter()
                    .filter(|c| c["seq"].as_i64() > Some(after))
                    .collect();
                let next = new.last().map_or(after, |c| c["seq"].as_i64().unwrap());
                Ok(Response::ok(
                    "workflow-runs",
                    Some(json!({"changes": new, "next": next})),
                ))
            }
            Request::Show(s) => {
                let name = s.target.unwrap().to_string();
                Ok(Response::ok(
                    "show",
                    Some(json!({"box": {"id": 9, "name": name, "branch": "moor/x"}})),
                ))
            }
            other => panic!("unexpected {other:?}"),
        };
        let mut post = |c: &str, t: &str, text: &str| {
            if self.broken.contains(&t) {
                return Err("channel_not_found".to_owned());
            }
            self.posts
                .borrow_mut()
                .push((c.into(), t.into(), text.into()));
            Ok(())
        };
        let mut fx = Fx {
            call: &mut call,
            post: &mut post,
            log: &|_| {},
            clock: || 7,
        };
        poll(db, "U1", &mut fx)
    }

    fn said(&self) -> Vec<String> {
        self.posts
            .borrow()
            .iter()
            .map(|(_, t, text)| format!("{t}: {text}"))
            .collect()
    }
}

#[test]
fn a_started_box_joins_its_thread_and_the_feed_lists_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "started", Some("sec-1"))]);
    world.poll(&db).unwrap();
    world.poll(&db).unwrap();
    let boxes = db.thread_boxes("C1", "1.0").unwrap();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].requested_by, "U1");
    assert!(matches!(boxes[0].state, BoxState::Bound { box_id, .. } if box_id.get() == 9));
    assert_eq!(db.fed_boxes().unwrap().len(), 1, "the feed sees it once");
    assert!(
        world.said().is_empty(),
        "the feed shows a Box, so the report adds no line"
    );
}

#[test]
fn a_finished_run_adopts_a_box_it_never_saw_start_and_leaves_the_words_to_the_feed() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "finished", Some("sec-1"))]);
    world.poll(&db).unwrap();
    assert_eq!(db.fed_boxes().unwrap().len(), 1);
    assert!(world.said().is_empty());
}

#[test]
fn a_timeout_a_refusal_and_a_disabled_workflow_each_get_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let disabled = json!({"kind": "disabled", "seq": 3, "workflow": "sec",
        "origin": {"front": "slack", "key": "C1/1.0"}, "reason": "3 failed runs in a row"});
    let world = World::new(vec![
        ours(1, "timed_out", Some("sec-1")),
        ours(2, "refused", None),
        disabled,
    ]);
    world.poll(&db).unwrap();
    world.poll(&db).unwrap();
    let said = world.said();
    assert_eq!(said.len(), 3, "{said:?}");
    assert!(
        said[0].contains("ran out of time")
            && said[0].contains("sec-1")
            && said[0].contains("close")
    );
    assert!(said[1].contains("repo not found") && said[1].contains("acme/api"));
    assert!(said[2].contains("disabled") && said[2].contains("3 failed runs in a row"));
    assert_eq!(
        db.fed_boxes().unwrap().len(),
        1,
        "the timed-out Box is kept in the thread"
    );
}

#[test]
fn a_streak_of_skips_is_said_once_and_a_later_streak_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let mut changes: Vec<Value> = (1..=3).map(|n| ours(n, "skipped_overlap", None)).collect();
    changes.push(ours(4, "started", Some("sec-1")));
    changes.push(ours(5, "skipped_overlap", None));
    let world = World::new(changes);
    world.poll(&db).unwrap();
    let said = world.said();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said
        .iter()
        .all(|s| s.contains("previous run is still open") || s.contains("still open")));
}

#[test]
fn the_cursor_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let world = World::new(vec![
        ours(1, "timed_out", Some("sec-1")),
        ours(2, "refused", None),
    ]);
    world.poll(&open(dir.path())).unwrap();
    world.poll(&open(dir.path())).unwrap();
    assert_eq!(*world.asked.borrow(), [Some(0), Some(2)]);
    assert_eq!(world.said().len(), 2, "nothing is said again");
}

#[test]
fn another_front_and_a_workflow_without_origin_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![
        run(1, ("herdr", Some("C1/1.0")), "refused", None),
        json!({"kind": "run", "seq": 2, "workflow": "sec", "repo": "acme/api", "due_at": 0,
            "outcome": "started", "box": "sec-1"}),
    ]);
    world.poll(&db).unwrap();
    assert!(world.said().is_empty());
    assert!(db.thread_boxes("C1", "1.0").unwrap().is_empty());
    world.poll(&db).unwrap();
    assert_eq!(
        *world.asked.borrow(),
        [Some(0), Some(2)],
        "the cursor moved past both"
    );
}

#[test]
fn a_failed_post_neither_holds_the_cursor_nor_blocks_another_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let mut elsewhere = run(2, ("slack", Some("C1/2.0")), "refused", None);
    elsewhere["workflow"] = json!("other");
    let mut world = World::new(vec![ours(1, "refused", None), elsewhere]);
    world.broken = vec!["1.0"];
    world.poll(&db).unwrap();
    assert_eq!(world.said().len(), 1, "thread 2.0 still hears");
    for _ in 0..GIVE_UP {
        world.poll(&db).unwrap();
    }
    assert!(
        db.owed_lines("U1").unwrap().is_empty(),
        "the line is given up on"
    );
    assert_eq!(
        world
            .asked
            .borrow()
            .iter()
            .skip(1)
            .copied()
            .collect::<Vec<_>>(),
        vec![Some(2); 5]
    );
    assert_eq!(world.said().len(), 1);
}

#[test]
fn a_controller_that_cannot_be_read_leaves_the_cursor_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let mut call = |_: Request| {
        Err(ClientError::Frame {
            kind: moor_core::controller::FrameErrorKind::Malformed,
            message: "down".into(),
        })
    };
    let mut post = |_: &str, _: &str, _: &str| Ok(());
    let mut fx = Fx {
        call: &mut call,
        post: &mut post,
        log: &|_| {},
        clock: || 7,
    };
    assert!(poll(&db, "U1", &mut fx).is_err());
    assert_eq!(db.workflow_cursor("U1").unwrap(), 0);
}
