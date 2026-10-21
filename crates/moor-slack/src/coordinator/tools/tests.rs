//! Tests for the coordinator's tools against a real `slack.db` and a scripted controller: the
//! requester gate, the journal written before a Box changes, and a rerun's refusal of repeats.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::cell::RefCell;
use std::path::Path;

use moor_client::ClientError;
use moor_core::name::{BoxId, BoxName};
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{run, Call, Reply, Scope};
use crate::coordinator::context::Action;
use crate::db::{BoxState, Db, InboxItem, ItemKind, SlackBox, SlackThread};

/// Box `one` of the thread, asked for by `U1`.
const ONE: &str = "slack-1-0-1";
/// Box `two` of the thread, asked for by `U2`.
const TWO: &str = "slack-1-0-2";

/// A database whose thread `1.0` of channel `C1` was started by `U1`, with Box [`ONE`] asked
/// for by `U1` and Box [`TWO`] by `U2`, both running.
fn two_authors(dir: &Path) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    for (id, (name, by)) in [(ONE, "U1"), (TWO, "U2")].into_iter().enumerate() {
        let boxed = SlackBox {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            box_name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "acme/api".into(),
            goal: format!("goal of {name}"),
            requested_by: by.into(),
            parent: None,
            state: BoxState::Starting,
        };
        db.insert_starting(&boxed, 1).unwrap();
        let id = BoxId::new(i64::try_from(id).unwrap() + 1);
        db.bind(&boxed.box_name, id, Some("moor/b")).unwrap();
    }
    db
}

/// Queues a message from `author` keyed by `source` and takes it as a turn; returns its id.
fn turn_of(db: &Db, author: &str, source: &str) -> i64 {
    let item = InboxItem {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        kind: ItemKind::Message,
        author: author.into(),
        text: "please".into(),
        source: source.into(),
    };
    db.queue(&item, 2).unwrap();
    db.take_turn(3).unwrap().unwrap().id
}

/// Records what the controller was sent and answers each request with `answer`.
struct Controller {
    sent: RefCell<Vec<Request>>,
}

impl Controller {
    fn new() -> Self {
        Self {
            sent: RefCell::new(Vec::new()),
        }
    }

    /// Runs `call` in the thread with this controller answering `ok` to everything.
    fn run(&self, db: &Db, call: &Call) -> Reply {
        self.run_with(db, call, &mut |r| {
            Ok(Response::ok(r.cmd(), Some(json!({}))))
        })
    }

    /// Runs `call` with `answer` deciding each reply.
    fn run_with(
        &self,
        db: &Db,
        call: &Call,
        answer: &mut dyn FnMut(&Request) -> Result<Response, ClientError>,
    ) -> Reply {
        let mut send = |request: Request| {
            let reply = answer(&request);
            self.sent.borrow_mut().push(request);
            reply
        };
        let mut scope = Scope {
            db,
            channel: "C1",
            thread_ts: "1.0",
            call: &mut send,
            clock: || 50,
            max_open_boxes: 1,
        };
        run(&mut scope, call)
    }

    /// The Box name of each `steer` sent.
    fn steered(&self) -> Vec<String> {
        let sent = self.sent.borrow();
        sent.iter()
            .filter_map(|r| match r {
                Request::Steer(s) => Some(s.target.to_string()),
                _ => None,
            })
            .collect()
    }
}

fn steer(name: &str, text: &str) -> Call {
    Call::SteerBox {
        box_name: name.into(),
        text: text.into(),
    }
}

fn close(name: &str) -> Call {
    Call::CloseBox {
        box_name: name.into(),
    }
}

/// The state of Box `name`.
fn state(db: &Db, name: &str) -> BoxState {
    let boxes = db.thread_boxes("C1", "1.0").unwrap();
    let found = boxes.into_iter().find(|b| b.box_name.as_str() == name);
    found.unwrap().state
}

#[test]
fn one_author_cannot_steer_or_close_the_other_authors_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let refused = controller.run(&db, &steer(TWO, "stop"));
    assert!(refused.is_error, "{refused:?}");
    assert_eq!(
        refused.text,
        format!("Only <@U2> can change Box `{TWO}`, and <@U2> wrote nothing in this turn.")
    );
    assert!(controller.run(&db, &close(TWO)).is_error);
    let sent = controller.run(&db, &steer(ONE, "go on"));
    assert_eq!(sent, Reply::done(format!("Sent to Box `{ONE}`.")));
    assert_eq!(controller.steered(), [ONE]);
    assert!(matches!(state(&db, TWO), BoxState::Bound { .. }));
    let journal = db.thread_actions("C1", "1.0").unwrap();
    assert_eq!(journal.len(), 1, "a refused call writes no journal row");
}

#[test]
fn the_other_author_can_change_their_own_box_in_a_turn_that_holds_their_message() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U2", "Ev1");
    let controller = Controller::new();
    assert!(controller.run(&db, &steer(ONE, "stop")).is_error);
    assert!(!controller.run(&db, &steer(TWO, "go on")).is_error);
    assert_eq!(controller.steered(), [TWO]);
}

#[test]
fn a_turn_that_holds_only_notices_cannot_change_a_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let raw = rusqlite::Connection::open(dir.path().join("slack.db")).unwrap();
    raw.execute_batch(
        "INSERT INTO slack_turns (id, channel, thread_ts, paid_by, started_at) \
         VALUES (9, 'C1', '1.0', 'U1', 3); \
         INSERT INTO slack_inbox (channel, thread_ts, kind, text, source, created_at, turn_id) \
         VALUES ('C1', '1.0', 'notice', '[moor] Box finished its turn.', 'n1', 2, 9);",
    )
    .unwrap();
    let controller = Controller::new();
    for call in [steer(ONE, "go on"), close(ONE)] {
        let reply = controller.run(&db, &call);
        assert!(reply.is_error, "{call:?} ran: {reply:?}");
        assert!(
            reply.text.contains("wrote nothing in this turn"),
            "{reply:?}"
        );
    }
    assert!(controller.sent.borrow().is_empty());
    let listed = controller.run(&db, &Call::ListBoxes);
    assert!(!listed.is_error, "list_boxes still answers: {listed:?}");
}

#[test]
fn no_running_turn_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let reply = Controller::new().run(&db, &close(ONE));
    assert_eq!(reply, Reply::refused("No turn of this thread is running."));
}

#[test]
fn a_box_of_another_thread_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run(&db, &steer("slack-9-9-1", "go"));
    assert_eq!(
        reply,
        Reply::refused("This thread has no Box `slack-9-9-1`. Call list_boxes for its Boxes.")
    );
    assert!(controller.sent.borrow().is_empty());
}

#[test]
fn steer_writes_its_journal_row_before_it_sends_and_the_outcome_after() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let seen = Db::open(&dir.path().join("slack.db")).unwrap();
    let mut before = Vec::new();
    let mut answer = |r: &Request| {
        before = seen.turn_actions(turn).unwrap();
        Ok(Response::ok(r.cmd(), None))
    };
    Controller::new().run_with(&db, &steer(ONE, "use tokio"), &mut answer);
    let row = |outcome: Option<&str>| Action {
        turn,
        requested_by: "U1".into(),
        tool: "steer_box".into(),
        box_name: ONE.into(),
        args: "use tokio".into(),
        outcome: outcome.map(str::to_owned),
    };
    assert_eq!(before, [row(None)], "the row is written before the steer");
    let sent = format!("sent to Box `{ONE}`");
    assert_eq!(db.turn_actions(turn).unwrap(), [row(Some(&sent))]);
}

#[test]
fn a_lost_steer_says_it_may_not_have_been_sent() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let mut answer = |_: &Request| {
        Err(ClientError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )))
    };
    let reply = Controller::new().run_with(&db, &steer(ONE, "go"), &mut answer);
    assert!(reply.is_error);
    assert!(
        reply
            .text
            .starts_with("Maybe not sent: the controller did not answer"),
        "{reply:?}"
    );
    let outcome = db.turn_actions(turn).unwrap()[0].outcome.clone().unwrap();
    assert!(
        outcome.starts_with("the controller did not answer"),
        "{outcome}"
    );
}

#[test]
fn close_marks_the_box_closing_with_its_journal_row_and_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run(&db, &close(ONE));
    assert_eq!(
        reply,
        Reply::done(format!(
            "Box `{ONE}` is closing. The thread will say when it has closed."
        ))
    );
    assert!(matches!(state(&db, ONE), BoxState::Closing { .. }));
    assert!(
        controller.sent.borrow().is_empty(),
        "the worker sends the close"
    );
    let journal = db.turn_actions(turn).unwrap();
    assert_eq!(journal[0].tool, "close_box");
    assert_eq!(
        journal[0].outcome.as_deref(),
        Some(&*format!("Box `{ONE}` is closing"))
    );
}

#[test]
fn close_of_a_box_still_starting_closing_or_closed_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    let starting = SlackBox {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        box_name: BoxName::parse_allow_bare(ONE).unwrap(),
        repo: "acme/api".into(),
        goal: "g".into(),
        requested_by: "U1".into(),
        parent: None,
        state: BoxState::Starting,
    };
    db.insert_starting(&starting, 1).unwrap();
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let still = controller.run(&db, &close(ONE));
    assert_eq!(
        still,
        Reply::refused(format!(
            "Box `{ONE}` is still starting. Try to close it once it has started."
        ))
    );
    db.bind(&starting.box_name, BoxId::new(1), None).unwrap();
    db.mark_closing(&starting.box_name).unwrap();
    let closing = controller.run(&db, &close(ONE));
    assert_eq!(closing, Reply::refused(format!("Box `{ONE}` is closing.")));
    db.settle_close(&starting.box_name, true).unwrap();
    let gone = controller.run(&db, &close(ONE));
    assert_eq!(gone, Reply::refused(format!("Box `{ONE}` is closed.")));
    assert!(db.thread_actions("C1", "1.0").unwrap().is_empty());
}

#[test]
fn a_rerun_refuses_to_repeat_what_its_cut_off_turn_did_and_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    assert!(!controller.run(&db, &steer(ONE, "use tokio")).is_error);
    assert!(!controller.run(&db, &close(ONE)).is_error);
    let lost = db.unended().unwrap().remove(0).row;
    db.rerun(&lost, 60).unwrap();
    let again = controller.run(&db, &steer(ONE, "use tokio"));
    assert!(again.is_error);
    assert_eq!(
        again.text,
        format!(
            "Your last turn already did this, so it is not done again. Turn {}, <@U1> asked. \
             steer_box Box `{ONE}`: use tokio. Result: sent to Box `{ONE}`.",
            lost.id
        )
    );
    let closed = controller.run(&db, &close(ONE));
    assert!(
        closed.text.starts_with("Your last turn already did this"),
        "{closed:?}"
    );
    db.settle_close(&BoxName::parse_allow_bare(ONE).unwrap(), false)
        .unwrap();
    let other = controller.run(&db, &steer(ONE, "and add a test"));
    assert!(!other.is_error, "a new action still runs: {other:?}");
}

#[test]
fn a_call_with_a_missing_argument_or_an_unknown_name_is_refused() {
    assert_eq!(
        Call::parse("steer_box", &json!({"box": "b"})),
        Err("steer_box needs a non-empty `text`.".to_owned())
    );
    assert_eq!(
        Call::parse("rm_rf", &json!({})),
        Err("There is no tool `rm_rf`.".to_owned())
    );
}

/// A database whose thread `1.0` of channel `C1` holds Box `one`, asked for by `U1` and
/// running, and Box `two`, asked for by `U2` and still starting.
fn two_boxes(dir: &Path) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    for (name, by) in [("slack-1-0-1", "U1"), ("slack-1-0-2", "U2")] {
        let boxed = SlackBox {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            box_name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "acme/api".into(),
            goal: format!("goal of {name}"),
            requested_by: by.into(),
            parent: None,
            state: BoxState::Starting,
        };
        db.insert_starting(&boxed, 1).unwrap();
    }
    let one = BoxName::parse_allow_bare("slack-1-0-1").unwrap();
    db.bind(&one, BoxId::new(1), Some("moor/b")).unwrap();
    db
}

/// Runs `call` in thread `1.0`, answering each request with `answer`; returns the reply and
/// what the controller was sent.
fn listed_with(
    db: &Db,
    call: &Call,
    answer: &dyn Fn(&Request) -> Response,
) -> (Reply, Vec<Request>) {
    let mut requests = Vec::new();
    let mut send = |request: Request| {
        let reply = answer(&request);
        requests.push(request);
        Ok(reply)
    };
    let mut scope = Scope {
        db,
        channel: "C1",
        thread_ts: "1.0",
        call: &mut send,
        clock: || 50,
        max_open_boxes: 1,
    };
    let reply = run(&mut scope, call);
    (reply, requests)
}

#[test]
fn list_boxes_shows_each_box_with_the_controllers_view_of_a_running_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let view =
        json!({"box": {"lifecycle": "running", "branch": "moor/b", "agent": {"state": "working"}}});
    let (reply, sent) = listed_with(&db, &Call::ListBoxes, &|r| {
        Response::ok(r.cmd(), Some(view.clone()))
    });
    let want = "- Box `slack-1-0-1` on acme/api, asked for by <@U1>: lifecycle running, agent \
                working, branch moor/b\n- Box `slack-1-0-2` on acme/api, asked for by <@U2>: \
                starting";
    assert_eq!(reply, Reply::done(want));
    let asked: Vec<_> = sent
        .iter()
        .map(|r| match r {
            Request::Show(s) => (s.target.as_ref().unwrap().to_string(), s.agent),
            other => panic!("only show is sent: {other:?}"),
        })
        .collect();
    assert_eq!(
        asked,
        [("slack-1-0-1".to_owned(), true)],
        "a starting Box is not asked about"
    );
}

#[test]
fn list_boxes_says_when_the_controller_did_not_answer() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let mut send = |_: Request| {
        Err(moor_client::ClientError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )))
    };
    let mut scope = Scope {
        db: &db,
        channel: "C1",
        thread_ts: "1.0",
        call: &mut send,
        clock: || 50,
        max_open_boxes: 1,
    };
    let reply = run(&mut scope, &Call::ListBoxes);
    assert!(
        reply.text.starts_with(
            "- Box `slack-1-0-1` on acme/api, asked for by <@U1>: the controller is not answering"
        ),
        "{reply:?}"
    );
}

#[test]
fn list_boxes_in_a_thread_with_no_box_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_boxes(dir.path());
    let mut send =
        |_: Request| -> Result<Response, moor_client::ClientError> { panic!("nothing is sent") };
    let mut scope = Scope {
        db: &db,
        channel: "C1",
        thread_ts: "9.9",
        call: &mut send,
        clock: || 50,
        max_open_boxes: 1,
    };
    assert_eq!(
        run(&mut scope, &Call::ListBoxes),
        Reply::done("This thread has no Box.")
    );
}

mod start;
