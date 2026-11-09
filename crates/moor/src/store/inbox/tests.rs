//! Tests for the inbox store, on the backend `MOOR_TEST_POSTGRES_URL` picks.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::event::{Event, SteerSent, TurnOutcome};
use moor_core::domain::inbox::{Delivery, InboxId, InboxKind, SendMark};
use moor_core::seam::clock::Clock as _;
use moor_core::{BoxId, LaunchId};

use crate::store::tests::{new_box, open};
use crate::store::{Store, StoreError};

const MARK: SendMark = SendMark {
    turn_before: Some(3),
    status_before: Some(42),
};

/// Makes `store` panic on a write to a Box's rows, as it does in an op that lacks the Box's turn.
#[expect(
    clippy::panic,
    reason = "stands in for the owners' write check, which panics"
)]
fn refuse_writes(store: &Store) {
    store.check_writes(Arc::new(|b| {
        panic!("Box {} written without its turn", b.get())
    }));
}

/// The `steered` event of a send of row `row`.
fn steered(row: InboxId) -> Event {
    Event::Steered(SteerSent {
        session: "agent-1".to_owned(),
        message: "go".to_owned(),
        submit_retried: false,
        turn_before: MARK.turn_before,
        status_before: MARK.status_before,
        inbox: Some(row.get()),
    })
}

/// A new launch of Box `id`'s agent.
fn launch(store: &Store, id: BoxId) -> LaunchId {
    LaunchId::new(store.begin_session(id, "agent").unwrap())
}

/// Each row as `<text>:<delivery>`.
fn texts(rows: &[crate::store::InboxRow]) -> Vec<String> {
    rows.iter()
        .map(|r| format!("{}:{}", r.text, r.delivery.as_str()))
        .collect()
}

#[test]
fn a_sender_appends_without_a_turn_and_the_oldest_row_is_next() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let other = new_box(&store, "b");
    refuse_writes(&store);
    let first = store
        .enqueue(id, InboxKind::Prompt, "fix it", "start")
        .unwrap();
    let second = store
        .enqueue(id, InboxKind::Steer, "and test", "U1 slack")
        .unwrap();
    assert!(second > first);
    let next = store.next_for(id).unwrap().unwrap();
    assert_eq!(
        (next.id, next.kind, next.text.as_str(), next.sender.as_str()),
        (first, InboxKind::Prompt, "fix it", "start")
    );
    assert_eq!(next.delivery, Delivery::Queued { attempts: 0 });
    assert_eq!(store.next_for(other).unwrap(), None);
}

#[test]
fn a_marked_row_is_in_doubt_until_confirmed_and_answering_it_ends_its_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = new_box(&store, "a");
    let s = launch(&store, id);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    let after = store.enqueue(id, InboxKind::Steer, "then", "u").unwrap();
    let doubt = Delivery::InDoubt {
        launch: s,
        attempts: 1,
    };
    assert_eq!(store.mark_sending(id, row, s).unwrap(), doubt);
    assert_eq!(store.next_for(id).unwrap().unwrap().delivery, doubt);
    clock.sleep(Duration::from_millis(5));
    let sent = Delivery::Sent {
        launch: s,
        at: clock.now().as_millis(),
        mark: MARK,
    };
    let (confirmed, steer) = store.confirm_sent(id, row, MARK, &steered(row)).unwrap();
    assert_eq!(confirmed, sent);
    assert_eq!(store.next_for(id).unwrap().unwrap().delivery, sent);
    assert!(store.answer(id, row, TurnOutcome::Idle).unwrap());
    assert_eq!(store.next_for(id).unwrap().unwrap().id, after);
    assert!(!store.answer(id, row, TurnOutcome::Idle).unwrap());
    let done: Vec<_> = store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "turn_done")
        .map(|e| e.payload)
        .collect();
    assert_eq!(
        done,
        [serde_json::json!({"steer": steer, "outcome": "idle", "inbox": row.get()})]
    );
}

#[test]
fn a_row_not_sent_is_not_answered() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    store.mark_sending(id, row, launch(&store, id)).unwrap();
    assert!(!store.answer(id, row, TurnOutcome::Idle).unwrap());
    assert_eq!(store.next_for(id).unwrap().unwrap().id, row);
    assert!(store
        .events(id, 100)
        .unwrap()
        .iter()
        .all(|e| e.kind != "turn_done"));
}

#[test]
fn a_send_to_a_gone_agent_returns_the_row_to_the_queue_and_still_counts() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let first = launch(&store, id);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    store.mark_sending(id, row, first).unwrap();
    assert_eq!(
        store.rollback(id, row).unwrap(),
        Delivery::Queued { attempts: 1 }
    );
    let second = launch(&store, id);
    assert_eq!(
        store.mark_sending(id, row, second).unwrap(),
        Delivery::InDoubt {
            launch: second,
            attempts: 2
        }
    );
    store.rollback(id, row).unwrap();
    assert_eq!(
        store.next_for(id).unwrap().unwrap().delivery,
        Delivery::Queued { attempts: 2 }
    );
    assert!(matches!(
        store.rollback(id, row),
        Err(StoreError::Illegal(_))
    ));
}

#[test]
fn a_send_the_agent_refused_before_typing_does_not_count() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let live = launch(&store, id);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    for _ in 0..3 {
        store.mark_sending(id, row, live).unwrap();
        assert_eq!(
            store.refuse(id, row).unwrap(),
            Delivery::Queued { attempts: 0 }
        );
    }
    assert!(matches!(store.refuse(id, row), Err(StoreError::Illegal(_))));
}

#[test]
fn answering_a_row_deletes_the_older_sent_rows_with_one_turn_done() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let live = launch(&store, id);
    store.enqueue(id, InboxKind::Steer, "waiting", "u").unwrap();
    let doubt = store.enqueue(id, InboxKind::Steer, "doubt", "u").unwrap();
    store.mark_sending(id, doubt, live).unwrap();
    let mut sent = Vec::new();
    for text in ["one", "two"] {
        let row = store.enqueue(id, InboxKind::Steer, text, "u").unwrap();
        store.mark_sending(id, row, live).unwrap();
        store.confirm_sent(id, row, MARK, &steered(row)).unwrap();
        sent.push(row);
    }
    store.enqueue(id, InboxKind::Steer, "three", "u").unwrap();
    assert!(store
        .answer(id, *sent.last().unwrap(), TurnOutcome::Idle)
        .unwrap());
    let left: Vec<_> = store.sent(id).unwrap();
    assert!(left.is_empty(), "{left:?}");
    let done = store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "turn_done")
        .count();
    assert_eq!(done, 1);
    assert_eq!(
        texts(&store.drop_all(id).unwrap()),
        ["waiting:queued", "doubt:in_doubt", "three:queued"],
        "rows not sent, older or newer, wait for their own send"
    );
}

#[test]
fn a_crash_then_a_gone_agent_never_gets_a_row_sent_three_times() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let first = launch(&store, id);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    store.mark_sending(id, row, first).unwrap();
    store.mark_sending(id, row, first).unwrap();
    store.rollback(id, row).unwrap();
    let second = launch(&store, id);
    for at in [first, second] {
        assert!(matches!(
            store.mark_sending(id, row, at),
            Err(StoreError::Illegal(_))
        ));
    }
}

#[test]
fn a_row_in_doubt_is_sent_once_more_to_its_launch_and_never_a_third_time() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let s = launch(&store, id);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    store.mark_sending(id, row, s).unwrap();
    let other = LaunchId::new(s.get() + 1);
    assert!(matches!(
        store.mark_sending(id, row, other),
        Err(StoreError::Illegal(_))
    ));
    assert_eq!(
        store.mark_sending(id, row, s).unwrap(),
        Delivery::InDoubt {
            launch: s,
            attempts: 2
        }
    );
    assert!(matches!(
        store.mark_sending(id, row, s),
        Err(StoreError::Illegal(_))
    ));
    store.confirm_sent(id, row, MARK, &steered(row)).unwrap();
    assert!(matches!(
        store.mark_sending(id, row, s),
        Err(StoreError::Illegal(_))
    ));
}

#[test]
fn cut_returns_exactly_the_rows_sent_or_in_doubt_on_an_older_launch() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let old = launch(&store, id);
    let sent = store.enqueue(id, InboxKind::Steer, "sent", "u").unwrap();
    let doubt = store.enqueue(id, InboxKind::Steer, "doubt", "u").unwrap();
    store.enqueue(id, InboxKind::Steer, "queued", "u").unwrap();
    let fresh = store.enqueue(id, InboxKind::Steer, "fresh", "u").unwrap();
    store.mark_sending(id, sent, old).unwrap();
    store.confirm_sent(id, sent, MARK, &steered(sent)).unwrap();
    store.mark_sending(id, doubt, old).unwrap();
    let other = new_box(&store, "b");
    let elsewhere = store.enqueue(other, InboxKind::Steer, "b", "u").unwrap();
    store
        .mark_sending(other, elsewhere, launch(&store, other))
        .unwrap();
    let new = launch(&store, id);
    store.mark_sending(id, fresh, new).unwrap();
    assert_eq!(
        texts(&store.cut(id, new).unwrap()),
        ["sent:sent", "doubt:in_doubt"]
    );
    assert_eq!(
        store.cut(id, old).unwrap(),
        [],
        "nothing is older than the first launch"
    );
}

#[test]
fn drop_all_deletes_and_returns_every_row_of_the_box_only() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let other = new_box(&store, "b");
    let row = store.enqueue(id, InboxKind::Steer, "one", "u").unwrap();
    store.enqueue(id, InboxKind::Note, "two", "moor").unwrap();
    store.enqueue(other, InboxKind::Steer, "kept", "u").unwrap();
    store.mark_sending(id, row, launch(&store, id)).unwrap();
    assert_eq!(
        texts(&store.drop_all(id).unwrap()),
        ["one:in_doubt", "two:queued"]
    );
    assert_eq!(store.next_for(id).unwrap(), None);
    assert_eq!(store.next_for(other).unwrap().unwrap().text, "kept");
}

#[test]
#[should_panic(expected = "written without its turn")]
fn an_update_without_a_turn_panics() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let s = launch(&store, id);
    refuse_writes(&store);
    let row = store.enqueue(id, InboxKind::Steer, "go", "u").unwrap();
    drop(store.mark_sending(id, row, s));
}
