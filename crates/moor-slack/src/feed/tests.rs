use moor_client::EventRecord;
use moor_core::name::BoxName;
use serde_json::json;

use super::*;
use crate::db::SlackThread;
use crate::identity::Team;
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
        clock: || 9,
        long_turn_secs: 1200,
    }
}

/// Records a thread with Box `slack-1-1` bound to [`ID`] and returns the Box row.
pub(super) fn bound_box(db: &Db) -> SlackBox {
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
        prompt: "fix it".into(),
        base: None,
        provider: None,
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

/// An event of `kind` for Box `box_id` at `cursor`.
fn event(cursor: i64, box_id: Option<BoxId>, kind: &str) -> Watched {
    Watched::Item(WatchItem::Event(EventRecord {
        cursor,
        box_name: Some("slack-1-1".into()),
        box_id,
        session: None,
        kind: kind.into(),
        payload: json!({}),
        ts: 0,
    }))
}

#[test]
fn a_new_connection_folds_afresh_and_acts_only_once_caught_up() {
    let mut fold = Fold::default();
    fold.take(Watched::Item(WatchItem::CaughtUp { cursor: 0 }));
    fold.take(event(1, Some(ID), "working"));
    fold.take(event(2, None, "idled"));
    assert!(fold.caught_up);
    let states: Vec<_> = fold
        .views
        .iter()
        .map(|(id, v)| (*id, v.state.clone()))
        .collect();
    assert_eq!(states, [(ID, status::State::Working)], "by Box id only");
    fold.take(Watched::Connected);
    assert!(!fold.caught_up, "a replay is not acted on until caught up");
    assert!(fold.views.is_empty());
}

#[test]
fn a_finished_turn_and_a_close_are_owed_lines_until_posted() {
    let mut fold = Fold::default();
    fold.take(event(1, Some(ID), "idled"));
    fold.take(event(2, Some(ID), "idled"));
    fold.take(event(3, Some(ID), "closed"));
    assert_eq!(fold.finished.get(&ID), Some(&vec![(1, None), (2, None)]));
    assert!(fold.closed.contains(&ID));
}

fn said(cursor: i64, text: &str) -> Watched {
    let Watched::Item(WatchItem::Event(mut e)) = event(cursor, Some(ID), "agent_said") else {
        unreachable!()
    };
    e.payload = json!({ "text": text });
    Watched::Item(WatchItem::Event(e))
}

#[test]
fn a_finished_turn_carries_the_message_its_agent_left_in_that_turn_only() {
    let mut fold = Fold::default();
    fold.take(said(1, "first"));
    fold.take(event(2, Some(ID), "idled"));
    fold.take(said(3, "dropped"));
    fold.take(event(4, Some(ID), "busy"));
    fold.take(event(5, Some(ID), "idled"));
    let want = vec![(2, Some("first".to_owned())), (5, None)];
    assert_eq!(fold.finished.get(&ID), Some(&want));
}

#[test]
fn a_closing_box_leaves_its_closed_line_to_the_close_path() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let mut row = bound_box(&db);
    db.mark_closing(&row.box_name, false).unwrap();
    row.state = BoxState::Closing {
        box_id: ID,
        branch: None,
        discard: false,
    };
    let mut fold = Fold::default();
    fold.take(event(1, Some(ID), "closed"));
    assert!(feed.lines(&db, &mut fold, &row), "nothing to post yet");
    assert!(fold.closed.contains(&ID), "kept for when the close settles");
    assert!(!db.has_notice(&row, BoxEvent::Closed, None).unwrap());
    let closing = db.closing().unwrap();
    assert_eq!(closing.len(), 1, "the close path settles it: {closing:?}");
}

#[test]
fn a_line_that_cannot_be_posted_queues_no_notice_and_is_tried_again() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let row = bound_box(&db);
    let mut fold = Fold::default();
    fold.take(event(1, Some(ID), "idled"));
    assert!(!feed.lines(&db, &mut fold, &row));
    assert_eq!(fold.finished.get(&ID), Some(&vec![(1, None)]), "still owed");
    assert!(!db.has_notice(&row, BoxEvent::Finished, Some(1)).unwrap());
}

#[test]
fn only_a_forced_sleep_and_a_relaunch_with_a_cause_owe_the_thread_a_line() {
    let mut fold = Fold::default();
    let at = |cursor, kind: &str, payload| {
        let Watched::Item(WatchItem::Event(mut e)) = event(cursor, Some(ID), kind) else {
            unreachable!()
        };
        e.payload = payload;
        Watched::Item(WatchItem::Event(e))
    };
    fold.take(at(1, "asleep", json!({"auto": false})));
    fold.take(at(
        2,
        "relaunched",
        json!({"old_pane": null, "cause": null}),
    ));
    fold.take(at(3, "asleep", json!({"auto": true})));
    fold.take(at(4, "relaunched", json!({"cause": "steer"})));
    let want = vec![
        (3, lifecycle::Note::AutoSlept),
        (4, lifecycle::Note::Relaunched),
    ];
    assert_eq!(fold.notes.get(&ID), Some(&want));
}

/// An identity of the Slack user `user` in the tests' team.
fn user(user: &str) -> Identity {
    Identity::parse(&format!("slack:T1/{user}")).unwrap()
}

#[test]
fn a_connection_resets_only_its_own_users_fold() {
    let mut folds = Folds::default();
    for who in ["U1", "U2"] {
        folds.take((user(who), Watched::Connected));
        folds.take((user(who), event(1, Some(ID), "working")));
        folds.take((user(who), Watched::Item(WatchItem::CaughtUp { cursor: 1 })));
    }
    folds.take((user("U2"), Watched::Connected));
    let mut caught_up: Vec<_> = folds.caught_up().map(|(who, _)| who.to_string()).collect();
    caught_up.sort();
    assert_eq!(
        caught_up,
        ["slack:T1/U1"],
        "U2 starts over, U1 keeps its fold"
    );
    let (_, kept) = folds.caught_up().next().unwrap();
    assert_eq!(kept.views.len(), 1, "U1's views survive U2's reconnect");
}

#[test]
fn only_the_caught_up_users_are_acted_on() {
    let mut folds = Folds::default();
    folds.take((user("U1"), event(1, Some(ID), "working")));
    assert_eq!(folds.caught_up().count(), 0, "a backlog is not acted on");
    folds.take((user("U1"), Watched::Item(WatchItem::CaughtUp { cursor: 1 })));
    folds.take((user("U2"), event(1, Some(ID), "working")));
    let who: Vec<_> = folds.caught_up().map(|(who, _)| who.to_string()).collect();
    assert_eq!(who, ["slack:T1/U1"]);
}
