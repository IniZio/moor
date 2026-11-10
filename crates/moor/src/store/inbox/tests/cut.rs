//! A new launch names to its agent each text the launch before it was cut off from, and never
//! sends one again.

use std::time::Duration;

use moor_core::domain::inbox::{cut_note, Delivery, InboxKind};
use moor_core::seam::clock::Clock as _;

use super::{launch, steered, texts, MARK};
use crate::store::tests::{new_box, open};
use crate::store::Store;

/// Each `steer_cut` payload of Box `id`, oldest first.
fn cuts(store: &Store, id: moor_core::BoxId) -> Vec<serde_json::Value> {
    store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "steer_cut")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_restored_launch_drops_rows_its_archive_holds_and_names_the_rest_first() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = new_box(&store, "a");
    let old = launch(&store, id);
    let held = store.enqueue(id, InboxKind::Steer, "held", "U1").unwrap();
    store.mark_sending(id, held, old).unwrap();
    store.confirm_sent(id, held, MARK, &steered(held)).unwrap();
    let archived = clock.now();
    clock.sleep(Duration::from_millis(5));
    let after = store.enqueue(id, InboxKind::Steer, "after", "U2").unwrap();
    store.mark_sending(id, after, old).unwrap();
    store
        .confirm_sent(id, after, MARK, &steered(after))
        .unwrap();
    let doubt = store
        .enqueue(id, InboxKind::Prompt, "doubt", "start")
        .unwrap();
    store.mark_sending(id, doubt, old).unwrap();
    let choice = store.enqueue(id, InboxKind::Choice, "2", "U1").unwrap();
    store.mark_sending(id, choice, old).unwrap();
    let queued = store.enqueue(id, InboxKind::Steer, "queued", "U3").unwrap();

    store
        .begin_launch_with(id, "agent", None, Some(archived))
        .unwrap();

    assert!(store.sent(id).unwrap().is_empty());
    let unsent = store.unsent(id).unwrap();
    let got: Vec<_> = unsent
        .iter()
        .map(|r| (r.id, r.kind, r.text.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (after, InboxKind::Note, cut_note("after")),
            (doubt, InboxKind::Note, cut_note("doubt")),
            (queued, InboxKind::Steer, "queued".to_owned()),
        ]
    );
    assert!(unsent
        .iter()
        .all(|r| r.delivery == Delivery::Queued { attempts: 0 }));
    assert_eq!(
        cuts(&store, id),
        [
            serde_json::json!({"inbox": after.get(), "sender": "U2"}),
            serde_json::json!({"inbox": doubt.get(), "sender": "start"}),
        ]
    );
}

#[test]
fn a_launch_after_the_agent_died_names_every_cut_text_and_sends_a_cut_note_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let other = new_box(&store, "b");
    let old = launch(&store, id);
    let sent = store.enqueue(id, InboxKind::Steer, "sent", "U1").unwrap();
    store.mark_sending(id, sent, old).unwrap();
    store.confirm_sent(id, sent, MARK, &steered(sent)).unwrap();
    let note = store
        .enqueue(id, InboxKind::Note, "moor says", "moor")
        .unwrap();
    store.mark_sending(id, note, old).unwrap();
    let elsewhere = store.enqueue(other, InboxKind::Steer, "b", "U9").unwrap();
    store
        .mark_sending(other, elsewhere, launch(&store, other))
        .unwrap();

    store.begin_launch(id, "agent-2").unwrap();

    assert_eq!(
        texts(&store.unsent(id).unwrap()),
        [
            format!("{}:queued", cut_note("sent")),
            "moor says:queued".to_owned()
        ]
    );
    assert_eq!(
        cuts(&store, id),
        [serde_json::json!({"inbox": sent.get(), "sender": "U1"})]
    );
    assert_eq!(texts(&store.unsent(other).unwrap()), ["b:in_doubt"]);
}

#[test]
fn a_note_cut_again_keeps_its_sends_so_it_goes_out_twice_at_most() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = new_box(&store, "a");
    let mut live = launch(&store, id);
    let note = store
        .enqueue(id, InboxKind::Note, "moor says", "moor")
        .unwrap();
    for sends in 1..=2 {
        store.mark_sending(id, note, live).unwrap();
        store.confirm_sent(id, note, MARK, &steered(note)).unwrap();
        live = launch(&store, id);
        assert_eq!(
            store.next_for(id).unwrap().unwrap().delivery,
            Delivery::Queued { attempts: sends }
        );
    }
    assert!(store.mark_sending(id, note, live).is_err());
}
