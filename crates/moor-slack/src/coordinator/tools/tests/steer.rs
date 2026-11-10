//! What a steer says in the thread when the Box takes it now, or keeps it queued.

use moor_core::response::Response;
use serde_json::json;

use super::{state, steer, turn_of, two_authors, Controller, ONE};
use crate::coordinator::tools::Reply;
use moor_core::name::BoxName;

use crate::db::{BoxState, Db, SlackBox, SlackThread};

#[test]
fn a_queued_steer_says_it_is_queued_and_why() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let value = json!({"box": ONE, "delivery": "queued", "cause": "waiting_for_room", "resource": "memory"});
    let reply = controller.run_with(&db, &steer(ONE, "go on"), &mut |r| {
        Ok(Response::ok(r.cmd(), Some(value.clone())))
    });
    assert_eq!(
        reply,
        Reply::done(format!(
            "Queued for Box `{ONE}` (waiting for room: memory)."
        ))
    );
}

#[test]
fn a_steer_to_a_starting_box_goes_to_moor_to_queue() {
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
        prompt: "g".into(),
        base: None,
        node: None,
        cold: false,
        requested_by: "U1".into(),
        parent: None,
        state: BoxState::Starting,
    };
    db.insert_starting(&starting, 1).unwrap();
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run(&db, &steer(ONE, "go on"));
    assert!(!reply.is_error, "{reply:?}");
    assert_eq!(controller.steered(), [ONE]);
    assert_eq!(state(&db, ONE), BoxState::Starting);
}
