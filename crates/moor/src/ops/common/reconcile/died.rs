//! A Box whose places died while its agent worked: put in the queue to resume on its own, until
//! it has died too often, and then left asleep and stuck.

use moor_core::decide::reconcile::died_of;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::deaths::{self, Tally, DEATH_WINDOW};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::BoxId;

use super::noted;
use crate::cx::Cx;
use crate::ops::common::node::now;
use crate::store::Store;

/// Records that working Box `id`, on hands `inc`, stopped as `reason` says. The caller holds the
/// Box's turn and has seen it Ready. It queues to resume first with its Session as it was, unless
/// its places died and have now done so [`deaths::DEATH_CAP`] times inside [`DEATH_WINDOW`]: then
/// it sleeps with its deaths as the reason, and nothing resumes it but a person.
pub(super) fn working(cx: &Cx, store: &Store, id: BoxId, inc: HandsInc, reason: &str) {
    let tally = died_of(reason).map(|why| {
        let earlier = store.recent_deaths(id).unwrap_or_default();
        let at = cx.clock().now().as_millis();
        (why, deaths::record(&earlier, at))
    });
    if let Some((why, tally)) = tally.as_ref().filter(|(_, t)| t.capped) {
        if noted(cx, store.apply(stuck(id, inc, why, tally))) {
            let _ = store.set_recent_deaths(id, &tally.kept);
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
        if let Some((_, tally)) = tally {
            let _ = store.set_recent_deaths(id, &tally.kept);
        }
    }
}

/// Sleeps Box `id` after its deaths: asleep, not queued, saying why, with the event that tells
/// its owner.
fn stuck(id: BoxId, inc: HandsInc, why: &str, tally: &Tally) -> Decision {
    let said = deaths::sentence(tally.kept.len(), why);
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
            count: tally.kept.len(),
            window_secs: DEATH_WINDOW.as_secs(),
            reason: why.to_owned(),
        })
}
