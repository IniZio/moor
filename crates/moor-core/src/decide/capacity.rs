//! The capacity decider: admission against `max_boxes` and promotion order.

use std::num::NonZeroU32;

use crate::domain::boxes::BoxState;
use crate::name::TaskId;

/// A Box some task holds, as the start op read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldBox {
    /// Name of the task holding the Box.
    pub holder: String,
    /// The Box's state.
    pub state: BoxState,
}

/// What the start op read before asking for a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityFacts {
    /// Configured `max_boxes`; `None` means no limit.
    pub limit: Option<NonZeroU32>,
    /// Every known Box, in any state.
    pub boxes: Vec<HeldBox>,
}

/// The verdict on a request for a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// A slot is free.
    Admitted,
    /// Every slot is taken.
    AtCapacity {
        /// A task holding a slot.
        holder: String,
        /// The configured limit.
        limit: NonZeroU32,
    },
}

/// Whether a Box in `state` takes a capacity slot.
#[must_use]
pub const fn counts(state: BoxState) -> bool {
    matches!(
        state,
        BoxState::Starting | BoxState::Ready | BoxState::Asleep
    )
}

/// Decides whether one more Box fits under the limit.
#[must_use]
pub fn admit(f: &CapacityFacts) -> Admission {
    let Some(limit) = f.limit else {
        return Admission::Admitted;
    };
    let mut used = 0usize;
    let mut first = None;
    for b in f.boxes.iter().filter(|b| counts(b.state)) {
        first.get_or_insert(b);
        used += 1;
    }
    match first {
        Some(b) if used >= limit.get() as usize => Admission::AtCapacity {
            holder: b.holder.clone(),
            limit,
        },
        _ => Admission::Admitted,
    }
}

/// Picks which waiting tasks to promote into `free` slots, oldest id first.
#[must_use]
pub fn promote_order(waiting: &[TaskId], free: u32) -> Vec<TaskId> {
    let mut ids = waiting.to_vec();
    ids.sort_unstable();
    ids.truncate(free as usize);
    ids
}

#[cfg(test)]
mod tests;
