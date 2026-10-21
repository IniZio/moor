use moor_client::EventRecord;
use moor_core::name::BoxName;
use serde_json::json;

use super::*;
use crate::db::SlackThread;

/// The id the tests give their Box.
const ID: BoxId = BoxId::new(7);

/// A feed whose Slack cannot be reached, so any post it tries fails.
fn feed(dir: &std::path::Path) -> Feed {
    Feed {
        db_path: dir.join("slack.db"),
        api: Arc::new(Api::new("http://127.0.0.1:9").unwrap()),
        bot_token: Arc::new(Secret::new(b"xoxb-test".to_vec())),
        log: Arc::new(|_| {}),
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
    let working = View {
        state: status::State::Working,
        machine: None,
    };
    assert_eq!(fold.views, HashMap::from([(ID, working)]), "by Box id only");
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
    assert_eq!(fold.finished.get(&ID), Some(&vec![1, 2]));
    assert!(fold.closed.contains(&ID));
}

#[test]
fn a_closing_box_leaves_its_closed_line_to_the_close_path() {
    let dir = tempfile::tempdir().unwrap();
    let feed = feed(dir.path());
    let db = Db::open(&feed.db_path).unwrap();
    let mut row = bound_box(&db);
    db.mark_closing(&row.box_name).unwrap();
    row.state = BoxState::Closing {
        box_id: ID,
        branch: None,
    };
    let mut fold = Fold::default();
    fold.take(event(1, Some(ID), "closed"));
    assert!(feed.lines(&db, &mut fold, &row), "nothing to post yet");
    assert!(fold.closed.contains(&ID), "kept for when the close settles");
    assert!(!db.has_notice(&row, BoxEvent::Closed, None).unwrap());
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
    assert_eq!(fold.finished.get(&ID), Some(&vec![1]), "still owed");
    assert!(!db.has_notice(&row, BoxEvent::Finished, Some(1)).unwrap());
}
