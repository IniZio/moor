//! Tests for Box rows: each Box of a thread has its own row, requester and state.

use moor_core::name::{BoxId, BoxName};

use super::super::{Db, SlackThread};
use super::{BoxState, SlackBox};

/// Box `name` of thread `1.1` in channel `C1`, asked for by `by`.
fn asked(name: &str, by: &str) -> SlackBox {
    SlackBox {
        channel: "C1".into(),
        thread_ts: "1.1".into(),
        box_name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "acme/api".into(),
        prompt: format!("prompt of {name}"),
        base: None,
        provider: None,
        requested_by: by.into(),
        parent: None,
        state: BoxState::Starting,
    }
}

/// A database whose thread `1.1` was started by `U1` and holds Boxes `one` by `U1` and `two`
/// by `U2`.
fn two_boxes(dir: &std::path::Path) -> (Db, SlackBox, SlackBox) {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.1".into(),
        owner: "U1".into(),
    };
    let (one, two) = (asked("one", "U1"), asked("two", "U2"));
    db.atomically(|| {
        db.insert_thread(&thread, 1)?;
        db.insert_starting(&one, 1)?;
        db.insert_starting(&two, 2)
    })
    .unwrap();
    (db, one, two)
}

#[test]
fn each_box_of_a_thread_keeps_its_own_requester_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let (db, one, two) = two_boxes(dir.path());
    db.bind(&two.box_name, BoxId::new(9), Some("moor/two"))
        .unwrap();
    let bound_two = SlackBox {
        state: BoxState::Bound {
            box_id: BoxId::new(9),
            branch: Some("moor/two".into()),
        },
        ..two
    };
    assert_eq!(
        db.thread_boxes("C1", "1.1").unwrap(),
        [one.clone(), bound_two]
    );
    assert_eq!(db.starting().unwrap(), [one]);
}

#[test]
fn forgetting_one_box_keeps_the_thread_and_its_other_box() {
    let dir = tempfile::tempdir().unwrap();
    let (db, one, two) = two_boxes(dir.path());
    db.forget_starting(&one.box_name).unwrap();
    assert_eq!(
        db.thread_boxes("C1", "1.1").unwrap(),
        std::slice::from_ref(&two)
    );
    assert_eq!(db.thread("C1", "1.1").unwrap().unwrap().owner, "U1");
    db.forget_starting(&two.box_name).unwrap();
    assert_eq!(db.thread_boxes("C1", "1.1").unwrap(), []);
    assert!(
        db.thread("C1", "1.1").unwrap().is_some(),
        "the thread keeps its turns"
    );
}
