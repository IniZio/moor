//! Tests for the reporter against a real `slack.db`, a scripted controller and a recording
//! Slack.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::cell::{Cell, RefCell};
use std::path::Path;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::{json, Value};

use super::{poll, Fx, GIVE_UP};
use crate::db::{BoxState, Db, SlackThread};
use crate::socket::Mention;
use crate::worker::record;

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

/// A run of workflow `sec` that fired for Box `planned`, which does not exist yet.
fn fired(seq: i64, planned: &str) -> Value {
    let mut v = ours(seq, "starting", None);
    v["planned_box"] = json!(planned);
    v
}

/// A refusal of the run that fired for Box `planned`.
fn refused_run(seq: i64, planned: &str) -> Value {
    let mut v = ours(seq, "refused", None);
    v["planned_box"] = json!(planned);
    v
}

fn ours(seq: i64, outcome: &str, boxed: Option<&str>) -> Value {
    run(seq, ("slack", Some("C1/1.0")), outcome, boxed)
}

/// The controller's stream and what Slack was sent.
struct World {
    changes: RefCell<Vec<Value>>,
    asked: RefCell<Vec<Option<i64>>>,
    /// `(channel, thread, text)`; no thread for a top-level message.
    posts: RefCell<Vec<(String, Option<String>, String)>>,
    logged: RefCell<Vec<String>>,
    /// Threads whose posts fail.
    broken: Vec<&'static str>,
    /// Top-level posts that fail before they go through.
    top_fails: Cell<u32>,
}

impl World {
    fn new(changes: Vec<Value>) -> Self {
        Self {
            changes: RefCell::new(changes),
            asked: RefCell::new(Vec::new()),
            posts: RefCell::new(Vec::new()),
            logged: RefCell::new(Vec::new()),
            broken: Vec::new(),
            top_fails: Cell::new(0),
        }
    }

    fn poll(&self, db: &Db) -> Result<(), String> {
        let mut call = |request: Request| match request {
            Request::WorkflowRuns(r) => {
                self.asked.borrow_mut().push(r.after);
                let after = r.after.unwrap_or(0);
                let all = self.changes.borrow();
                let new: Vec<_> = all
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
        let mut post = |c: &str, t: Option<&str>, text: &str| {
            let broken = t.is_some_and(|t| self.broken.contains(&t));
            let top_fails = t.is_none() && self.top_fails.get() > 0;
            if broken || top_fails {
                self.top_fails.set(self.top_fails.get().saturating_sub(1));
                return Err("channel_not_found".to_owned());
            }
            let mut posts = self.posts.borrow_mut();
            posts.push((c.into(), t.map(str::to_owned), text.into()));
            Ok(format!("100.{}", posts.len()))
        };
        let log = |line: &str| self.logged.borrow_mut().push(line.to_owned());
        let mut fx = Fx {
            call: &mut call,
            post: &mut post,
            log: &log,
            clock: || 7,
        };
        poll(db, "U1", &mut fx)
    }

    /// Each post as `thread: text`, `top` for a top-level message.
    fn said(&self) -> Vec<String> {
        self.posts
            .borrow()
            .iter()
            .map(|(_, t, text)| format!("{}: {text}", t.as_deref().unwrap_or("top")))
            .collect()
    }

    /// The top-level posts.
    fn tops(&self) -> Vec<String> {
        let posts = self.posts.borrow();
        let tops = posts.iter().filter(|(_, t, _)| t.is_none());
        tops.map(|(_, _, text)| text.clone()).collect()
    }
}

/// The reply of `user` in thread `ts` of channel `C1`.
fn reply(user: &str, ts: &str) -> Mention {
    Mention {
        event_id: format!("Ev-{user}"),
        channel: "C1".into(),
        user: user.into(),
        text: "look at the diff".into(),
        ts: "200.0".into(),
        thread_ts: Some(ts.into()),
        reply: true,
    }
}

#[test]
fn a_started_run_opens_its_own_thread_and_the_origin_thread_hears_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "started", Some("sec-1"))]);
    world.poll(&db).unwrap();
    let tops = world.tops();
    assert_eq!(tops.len(), 1, "{tops:?}");
    let missing = [
        "<@U1>",
        "`sec`",
        "`acme/api`",
        "https://slack.com/archives/C1/p10",
    ]
    .into_iter()
    .filter(|want| !tops[0].contains(want))
    .collect::<Vec<_>>();
    assert!(missing.is_empty(), "{missing:?} not in {}", tops[0]);
    assert_eq!(world.posts.borrow().len(), 1, "nothing else is posted");
    assert!(db.thread_boxes("C1", "1.0").unwrap().is_empty());
    assert_adopted_in_new_thread(&db);
}

/// Box `sec-1` is bound on the new thread, owned by `U1`, and the feed lists it once.
fn assert_adopted_in_new_thread(db: &Db) {
    let boxes = db.thread_boxes("C1", "100.1").unwrap();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].requested_by, "U1");
    assert!(matches!(boxes[0].state, BoxState::Bound { box_id, .. } if box_id.get() == 9));
    assert_eq!(db.thread("C1", "100.1").unwrap().unwrap().owner, "U1");
    assert_eq!(db.fed_boxes().unwrap().len(), 1, "the feed sees it once");
}

#[test]
fn each_run_gets_a_thread_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![
        ours(1, "started", Some("sec-1")),
        ours(2, "started", Some("sec-2")),
    ]);
    world.poll(&db).unwrap();
    assert_eq!(world.tops().len(), 2);
    let one = db.thread_boxes("C1", "100.1").unwrap();
    let two = db.thread_boxes("C1", "100.2").unwrap();
    assert_eq!(
        (one[0].box_name.as_str(), two[0].box_name.as_str()),
        ("sec-1", "sec-2")
    );
}

#[test]
fn a_run_that_finished_unseen_gets_a_thread_and_the_feed_keeps_the_words() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "finished", Some("sec-1"))]);
    world.poll(&db).unwrap();
    assert_eq!(db.thread_boxes("C1", "100.1").unwrap().len(), 1);
    assert_eq!(
        world.posts.borrow().len(),
        1,
        "only the thread's first message"
    );
}

#[test]
fn a_failed_first_post_holds_nothing_and_a_later_one_makes_one_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![
        ours(1, "started", Some("sec-1")),
        ours(2, "refused", None),
    ]);
    world.top_fails.set(2);
    world.poll(&db).unwrap();
    assert_eq!(db.workflow_cursor("U1").unwrap(), 2, "the cursor moved on");
    assert!(db.thread_boxes("C1", "100.1").unwrap().is_empty());
    assert_eq!(
        world.said().len(),
        1,
        "the refusal still reaches the origin"
    );
    world.poll(&db).unwrap();
    world.poll(&db).unwrap();
    assert_eq!(world.tops().len(), 1);
    assert_eq!(db.thread_boxes("C1", "100.2").unwrap().len(), 1);
    assert_eq!(*world.asked.borrow(), [Some(0), Some(2), Some(2)]);
}

#[test]
fn a_run_thread_given_up_on_is_logged_with_its_box_and_never_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![
        ours(1, "started", Some("sec-1")),
        ours(2, "finished", Some("sec-1")),
    ]);
    world.top_fails.set(1000);
    for _ in 0..GIVE_UP + 3 {
        world.poll(&db).unwrap();
    }
    assert_eq!(
        1000 - world.top_fails.get(),
        u32::try_from(GIVE_UP).unwrap()
    );
    let logged = world.logged.borrow();
    let dropped: Vec<_> = logged.iter().filter(|l| l.contains("dropped")).collect();
    assert_eq!(dropped.len(), 1, "{logged:?}");
    assert!(dropped[0].contains("sec-1") && dropped[0].contains("sec"));
    assert!(
        db.thread_boxes("C1", "1.0").unwrap().is_empty(),
        "left unadopted"
    );
}

#[test]
fn polling_again_after_success_repeats_nothing_even_with_a_reopened_db() {
    let dir = tempfile::tempdir().unwrap();
    let world = World::new(vec![
        ours(1, "started", Some("sec-1")),
        ours(2, "finished", Some("sec-1")),
    ]);
    world.poll(&open(dir.path())).unwrap();
    world.poll(&open(dir.path())).unwrap();
    world.poll(&open(dir.path())).unwrap();
    assert_eq!(world.posts.borrow().len(), 1);
    assert_eq!(open(dir.path()).fed_boxes().unwrap().len(), 1);
}

#[test]
fn the_owner_reaches_the_coordinator_from_the_run_thread_and_others_get_the_usual_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    World::new(vec![ours(1, "started", Some("sec-1"))])
        .poll(&db)
        .unwrap();
    assert_eq!(record(&reply("U1", "100.1"), &db, 9).unwrap(), None);
    let inbox = db.inbox("C1", "100.1").unwrap();
    assert_eq!(inbox.last().unwrap().text, "look at the diff");
    assert_eq!(
        db.thread_boxes("C1", "100.1").unwrap()[0].box_name.as_str(),
        "sec-1"
    );
    let other = record(&reply("U2", "100.1"), &db, 9).unwrap().unwrap();
    assert!(format!("{other:?}").contains("Only <@U1> can direct this thread"));
}

#[test]
fn a_timeout_goes_to_the_run_thread_and_the_rest_go_to_the_origin() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let disabled = json!({"kind": "disabled", "seq": 4, "workflow": "sec",
        "origin": {"front": "slack", "key": "C1/1.0"}, "reason": "3 failed runs in a row"});
    let world = World::new(vec![
        ours(1, "started", Some("sec-1")),
        ours(2, "timed_out", Some("sec-1")),
        ours(3, "refused", None),
        disabled,
    ]);
    world.poll(&db).unwrap();
    world.poll(&db).unwrap();
    let said = world.said();
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(said[0].starts_with("top: "));
    let at = |needle: &str| said.iter().find(|s| s.contains(needle)).unwrap().clone();
    assert!(at("ran out of time").starts_with("100.1: "));
    assert!(at("ran out of time").contains("close"));
    assert!(at("repo not found").starts_with("1.0: "));
    assert!(at("3 failed runs in a row").starts_with("1.0: "));
}

#[test]
fn a_timeout_seen_before_any_start_waits_for_the_run_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "timed_out", Some("sec-1"))]);
    world.top_fails.set(1);
    world.poll(&db).unwrap();
    assert!(world.said().is_empty(), "the line waits for the thread");
    world.poll(&db).unwrap();
    let said = world.said();
    assert!(
        said[0].starts_with("top: ") && said[1].starts_with("100.1: "),
        "{said:?}"
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
    let origin: Vec<_> = world
        .said()
        .into_iter()
        .filter(|s| s.starts_with("1.0: "))
        .collect();
    assert_eq!(origin.len(), 2, "{origin:?}");
    assert!(origin.iter().all(|s| s.contains("still open")));
}

#[test]
fn the_cursor_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let world = World::new(vec![
        ours(1, "refused", None),
        run(2, ("slack", Some("C1/1.0")), "started", Some("sec-1")),
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
        run(1, ("herdr", Some("C1/1.0")), "started", Some("sec-1")),
        json!({"kind": "run", "seq": 2, "workflow": "sec", "repo": "acme/api", "due_at": 0,
            "outcome": "started", "box": "sec-2"}),
    ]);
    world.poll(&db).unwrap();
    assert!(world.said().is_empty());
    assert!(db.fed_boxes().unwrap().is_empty());
    world.poll(&db).unwrap();
    assert_eq!(
        *world.asked.borrow(),
        [Some(0), Some(2)],
        "the cursor moved past both"
    );
}

#[test]
fn a_failed_line_neither_holds_the_cursor_nor_blocks_another_thread() {
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
    let mut post = |_: &str, _: Option<&str>, _: &str| Ok(String::new());
    let mut fx = Fx {
        call: &mut call,
        post: &mut post,
        log: &|_| {},
        clock: || 7,
    };
    assert!(poll(&db, "U1", &mut fx).is_err());
    assert_eq!(db.workflow_cursor("U1").unwrap(), 0);
}

/// The posts so far, then again after polling with a reopened db, which must add none.
fn assert_repolling_repeats_nothing(dir: &std::path::Path, world: &World) {
    let before = world.posts.borrow().len();
    let boxes = open(dir).fed_boxes().unwrap().len();
    world.poll(&open(dir)).unwrap();
    world.poll(&open(dir)).unwrap();
    assert_eq!(world.posts.borrow().len(), before);
    assert_eq!(open(dir).fed_boxes().unwrap().len(), boxes);
}

#[test]
fn a_run_that_has_only_fired_gets_its_thread_with_no_box_yet() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![fired(1, "sec-1")]);
    world.poll(&db).unwrap();
    let tops = world.tops();
    assert_eq!(tops.len(), 1, "{tops:?}");
    let missing = [
        "<@U1>",
        "`sec`",
        "`acme/api`",
        "https://slack.com/archives/C1/p10",
    ]
    .into_iter()
    .filter(|want| !tops[0].contains(want))
    .collect::<Vec<_>>();
    assert!(missing.is_empty(), "{missing:?} not in {}", tops[0]);
    assert!(!tops[0].contains("works here"), "no Box is promised");
    assert_eq!(db.thread("C1", "100.1").unwrap().unwrap().owner, "U1");
    assert!(db.fed_boxes().unwrap().is_empty());
    assert_repolling_repeats_nothing(dir.path(), &world);
}

#[test]
fn the_box_that_starts_later_joins_the_thread_without_a_second_post() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![fired(1, "sec-1")]);
    world.poll(&db).unwrap();
    *world.changes.borrow_mut() = vec![ours(2, "started", Some("sec-1"))];
    world.poll(&db).unwrap();
    assert_eq!(world.posts.borrow().len(), 1);
    assert_adopted_in_new_thread(&db);
    *world.changes.borrow_mut() = vec![ours(3, "finished", Some("sec-1"))];
    assert_repolling_repeats_nothing(dir.path(), &world);
    assert_eq!(world.posts.borrow().len(), 1);
}

#[test]
fn a_box_that_starts_before_the_first_post_goes_through_still_ends_in_one_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![fired(1, "sec-1")]);
    world.top_fails.set(1);
    world.poll(&db).unwrap();
    assert!(world.posts.borrow().is_empty());
    *world.changes.borrow_mut() = vec![ours(2, "started", Some("sec-1"))];
    world.poll(&db).unwrap();
    assert_eq!(world.tops().len(), 1);
    assert_adopted_in_new_thread(&db);
    assert_eq!(db.workflow_cursor("U1").unwrap(), 2);
    assert_repolling_repeats_nothing(dir.path(), &world);
}

#[test]
fn a_refusal_after_the_run_fired_goes_in_its_thread_even_after_an_earlier_streak() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![ours(1, "refused", None), fired(2, "sec-1")]);
    world.poll(&db).unwrap();
    *world.changes.borrow_mut() = vec![ours(1, "refused", None), refused_run(3, "sec-1")];
    world.poll(&db).unwrap();
    let said = world.said();
    assert_eq!(said.len(), 3, "{said:?}");
    assert!(said[0].starts_with("top: "));
    assert!(said[1].starts_with("1.0: ") && said[1].contains("repo not found"));
    assert!(
        said[2].starts_with("100.1: ") && said[2].contains("could not start"),
        "{said:?}"
    );
    assert_repolling_repeats_nothing(dir.path(), &world);
}

#[test]
fn a_refusal_with_no_thread_goes_to_the_origin_once_per_streak() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let world = World::new(vec![
        refused_run(1, "sec-1"),
        refused_run(2, "sec-2"),
        refused_run(3, "sec-3"),
    ]);
    world.poll(&db).unwrap();
    let said = world.said();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("1.0: ") && said[0].contains("will not say this again"));
    assert_repolling_repeats_nothing(dir.path(), &world);
}

#[test]
fn a_store_at_v6_keeps_its_owed_and_opened_run_threads_through_v7() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let v6: Vec<_> = crate::db::schema_for_tests()
        .into_iter()
        .filter(|m| m.version() <= 6)
        .collect();
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    moor_migrate::migrate(&mut conn, &v6, crate::db::legacy_for_tests).unwrap();
    conn.execute_batch(
        "INSERT INTO slack_threads (channel, thread_ts, owner, created_at) VALUES ('C1', '1.0', 'U1', 1), ('C1', '100.1', 'U1', 1);
         INSERT INTO slack_boxes (box_name, channel, thread_ts, n, repo, prompt, requested_by, state, box_id, created_at)
           VALUES ('sec-1', 'C1', '100.1', 1, 'acme/api', 'p', 'U1', 'bound', 9, 1);
         INSERT INTO workflow_run_threads (box_name, owner, workflow, repo, channel, origin_ts, box_id, branch, attempts, opened)
           VALUES ('sec-1', 'U1', 'sec', 'acme/api', 'C1', '1.0', 9, 'b', 0, 1),
                  ('sec-2', 'U1', 'sec', 'acme/api', 'C1', '1.0', 5, NULL, 2, 0);",
    )
    .unwrap();
    drop(conn);
    let db = open(dir.path());
    assert_eq!(
        db.run_thread_at("sec-1").unwrap(),
        Some(("C1".into(), "100.1".into()))
    );
    assert_eq!(db.run_thread_at("sec-2").unwrap(), None);
    assert_eq!(db.run_thread_has_box("sec-2").unwrap(), Some(true));
    let owed = db.owed_run_threads("U1", 5).unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(
        (owed[0].0.box_name.as_str(), owed[0].0.box_id, owed[0].1),
        ("sec-2", Some(5), 2)
    );
    let mut idless = owed[0].0.clone();
    idless.box_name = "sec-3".into();
    idless.box_id = None;
    db.owe_run_thread(&idless).unwrap();
    assert_eq!(db.run_thread_has_box("sec-3").unwrap(), Some(false));
}
