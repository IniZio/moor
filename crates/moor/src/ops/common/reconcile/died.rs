//! A Box whose places died while its agent worked: put in the queue to resume on its own after a
//! stop with no word of why, and left asleep and stuck after a death.

use moor_core::decide::reconcile::died_of;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::deaths;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::BoxId;

use super::noted;
use crate::cx::Cx;
use crate::ops::common::node::now;
use crate::store::Store;

/// Records that working Box `id`, on hands `inc`, stopped as `reason` says. The caller holds the
/// Box's turn and has seen it Ready. Its places dying leaves it asleep with the way they ended
/// as the reason, and nothing resumes it but a person. Any other stop queues it to resume first
/// with its Session as it was.
pub(super) fn working(cx: &Cx, store: &Store, id: BoxId, inc: HandsInc, reason: &str) {
    if let Some(why) = died_of(reason) {
        if noted(cx, store.apply(stuck(id, inc, why))) {
            let _ = store.set_resume_first(id, None);
            return;
        }
    }
    let suspend = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Suspend {
            reason: reason.to_owned(),
        })
        .hands_input(inc, HandsInput::Sleep)
        .set(Fields {
            state_reason: Field::Set(reason.to_owned()),
            ..Fields::default()
        });
    if noted(cx, store.apply(suspend)) {
        let _ = store.owe_suspended(id, now(cx), true);
    }
}

/// Sleeps Box `id` after its death: asleep, not queued, saying why, with the event that tells
/// its owner.
fn stuck(id: BoxId, inc: HandsInc, why: &str) -> Decision {
    let said = deaths::sentence(why);
    Decision::new(id)
        .lifecycle(BoxLifecycleInput::Sleep {
            auto: false,
            reason: Some(said.clone()),
        })
        .hands_input(inc, HandsInput::Sleep)
        .set(Fields {
            state_reason: Field::Set(said),
            ..Fields::default()
        })
        .event(ExtraEvent::DeathCapped {
            reason: why.to_owned(),
        })
}
