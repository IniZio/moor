//! A text sent through a Box's inbox, its turn still open.

use moor_core::domain::event::{Event, SteerSent};
use moor_core::domain::inbox::{InboxId, InboxKind, SendMark};
use moor_core::BoxId;

use crate::store::Store;

/// Sends "go" to the live agent of Box `id` through its inbox, as a steer marked at
/// `turn_before` ended turns and a status set at `status_before`.
pub(crate) fn sent(
    store: &Store,
    id: BoxId,
    turn_before: Option<u64>,
    status_before: Option<i64>,
) -> InboxId {
    let launch = store.live_launch(id).unwrap().unwrap();
    let row = store.enqueue(id, InboxKind::Steer, "go", "test").unwrap();
    store.mark_sending(id, row, launch.id).unwrap();
    let steered = Event::Steered(SteerSent {
        session: launch.session_ref,
        message: "go".to_owned(),
        submit_retried: false,
        turn_before,
        status_before,
        inbox: Some(row.get()),
    });
    let mark = SendMark {
        turn_before,
        status_before,
    };
    store.confirm_sent(id, row, mark, &steered).unwrap();
    row
}
