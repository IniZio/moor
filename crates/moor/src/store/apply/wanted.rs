//! The wanted action and the effect in flight: the rules a decision's changes to them follow.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted, WantedKind};
use moor_core::seam::clock::Ts;
use moor_schema::tables::BoxState;

use super::sql::Sets;
use crate::store::StoreError;

/// What a Box held before a decision: its wanted action, and its effect in flight with the
/// attempt it is on.
#[derive(Debug, Clone, Default)]
pub(super) struct Held {
    /// The wanted action.
    pub(super) wanted: Option<Wanted>,
    /// The effect in flight and its attempt.
    pub(super) effect: Option<(Effect, u32)>,
}

/// Column assignments for the wanted action and effect `d` leaves on a Box that held `held`.
///
/// A raise lands only when it outranks what the Box wants, so ops that race resolve the same way
/// whichever writes first. The lifecycle step a wanted action ends in then clears it, and any
/// weaker one it fulfils: a close clears every one, a suspend a suspend or sleep, a sleep a sleep.
/// A close or rejection also ends the effect in flight, as nothing goes on in a closed Box.
pub(super) fn sets(held: &Held, d: &Decision, now: Ts) -> Result<Sets<BoxState>, StoreError> {
    let mut sets = want_sets(held, d, now);
    let effect = if closes(d) {
        EffectChange::End
    } else {
        d.effect
    };
    sets.extend(effect_sets(held, effect, now)?);
    Ok(sets)
}

/// Whether `d` closes or rejects the Box, which leaves nothing in flight on it.
const fn closes(d: &Decision) -> bool {
    matches!(
        d.lifecycle_input,
        Some(BoxLifecycleInput::Close { .. } | BoxLifecycleInput::Rejected { .. })
    )
}

/// The wanted columns `d` writes, if any.
fn want_sets(held: &Held, d: &Decision, now: Ts) -> Sets<BoxState> {
    let raised = match &d.wanted {
        WantChange::Raise(w) if w.outranks(held.wanted.as_ref()) => Some(Some(w)),
        WantChange::Done(kind) if held.wanted.as_ref().is_some_and(|w| w.kind() == *kind) => {
            Some(None)
        }
        WantChange::Keep | WantChange::Raise(_) | WantChange::Done(_) => None,
    };
    let after = raised.unwrap_or(held.wanted.as_ref());
    let fulfils = match d.lifecycle_input {
        _ if closes(d) => Some(WantedKind::Close),
        Some(BoxLifecycleInput::Suspend { .. }) => Some(WantedKind::Suspend),
        Some(BoxLifecycleInput::Sleep { .. }) => Some(WantedKind::Sleep),
        _ => None,
    };
    let cleared = after.is_some_and(|w| fulfils.is_some_and(|k| w.kind() <= k));
    match (raised, cleared) {
        (_, true) => wanted_columns(None, now),
        (Some(w), false) => wanted_columns(w, now),
        (None, false) => Vec::new(),
    }
}

/// The wanted columns holding `w`, all null for none.
fn wanted_columns(w: Option<&Wanted>, now: Ts) -> Sets<BoxState> {
    vec![
        (BoxState::Wanted, w.map(|w| w.kind().as_str()).into()),
        (
            BoxState::WantedForce,
            w.map(|w| i64::from(w.force())).into(),
        ),
        (
            BoxState::WantedReason,
            w.and_then(Wanted::reason).map(str::to_owned).into(),
        ),
        (BoxState::WantedAt, w.map(|_| now.as_millis()).into()),
    ]
}

/// The effect columns `change` writes over `held`; beginning an effect over one in flight, or
/// retrying none, is a bug in the op that owns the Box.
fn effect_sets(held: &Held, change: EffectChange, now: Ts) -> Result<Sets<BoxState>, StoreError> {
    let (effect, attempt) = match (change, held.effect) {
        (EffectChange::Keep, _) => return Ok(Vec::new()),
        (EffectChange::Next(e), Some((Effect::Run { .. }, _))) if e.run().is_none() => {
            return Ok(Vec::new());
        }
        (EffectChange::Finish(e), held) if held.is_none_or(|(on, _)| on.as_str() != e.as_str()) => {
            return Ok(Vec::new());
        }
        (EffectChange::Begin(e), None) | (EffectChange::Next(e), _) => (Some(e), 1),
        (EffectChange::Begin(e), Some((on, _))) => {
            return Err(StoreError::Invalid(format!(
                "cannot begin {} while {} is in flight",
                e.as_str(),
                on.as_str()
            )));
        }
        (EffectChange::Again, Some((e, n))) => (Some(e), n.saturating_add(1)),
        (EffectChange::Again, None) => {
            return Err(StoreError::Invalid(
                "no effect in flight to try again".into(),
            ));
        }
        (EffectChange::End | EffectChange::Finish(_), _) => (None, 0),
    };
    Ok(vec![
        (BoxState::Effect, effect.map(|e| e.as_str()).into()),
        (
            BoxState::EffectAttempt,
            effect.map(|_| i64::from(attempt)).into(),
        ),
        (
            BoxState::EffectRun,
            effect
                .and_then(|e| e.run())
                .map(moor_core::RunId::get)
                .into(),
        ),
        (BoxState::EffectAt, effect.map(|_| now.as_millis()).into()),
    ])
}
