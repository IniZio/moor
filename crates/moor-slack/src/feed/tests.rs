use moor_client::EventRecord;
use serde_json::json;

use super::*;

/// The id the tests give their Box.
const ID: BoxId = BoxId::new(7);

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
