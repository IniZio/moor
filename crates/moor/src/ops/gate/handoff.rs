//! The hand-over: a gate passed and the next one waits for the agent, which is told once.
//!
//! The line that passed the gate is the last line written, since every line of the gate in hand
//! names that gate. A steer after it, from here or from a person, has told the agent already.

use moor_core::domain::event::Origin;
use moor_core::gate::{status, GateLine, GateNext, GateStatus, Gates};
use moor_core::request::BoxTarget;
use moor_core::SteerReq;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::quiet::may_tell;
use crate::ops::steer;
use crate::store::{BoxRow, Store};

/// Steers the agent of `row` that the gate before the one in hand has passed, when the gate in hand
/// waits for the agent, the agent is quiet and nothing was steered since. Whether it did.
///
/// # Errors
/// A store failure, or the steer being refused.
pub(super) fn hand_over(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (gates, lines): (&Gates, &[GateLine]),
) -> Result<bool, CommandError> {
    let GateStatus::Open {
        gate,
        next: GateNext::Agent,
        ..
    } = status(gates, lines)
    else {
        return Ok(false);
    };
    if lines.last().is_none_or(|l| l.gate() == &gate) {
        return Ok(false);
    }
    let Some(at) = gates.iter().position(|g| g.name == gate) else {
        return Ok(false);
    };
    let (Some(passed), Some(waiting)) =
        (at.checked_sub(1).and_then(|p| gates.get(p)), gates.get(at))
    else {
        return Ok(false);
    };
    if !may_tell(cx, store, row)? {
        return Ok(false);
    }
    let wrote = store.gate_line_at(row.id, i64::try_from(lines.len()).unwrap_or(i64::MAX))?;
    let steered = store.last_event_at(row.id, &["steered"])?;
    if steered.zip(wrote).is_some_and(|(s, w)| s >= w) {
        return Ok(false);
    }
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(waiting.handoff_steer(&passed.name)),
        menu: None,
        interrupt: false,
        ask: None,
    };
    cx.for_box(row.user, || steer::run_as(cx, &req, Origin::Moor))
        .map(|_| true)
}
