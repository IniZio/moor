//! Pure domain model: task and box state machines, events and store decisions.

pub mod boxes;
pub mod decision;
pub mod event;
pub mod task;

/// A state machine input that is not legal from the current state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition: {input} from {from}")]
pub struct Illegal {
    /// The current state name, or `none` when no state exists yet.
    pub from: String,
    /// The rejected input name.
    pub input: String,
}
