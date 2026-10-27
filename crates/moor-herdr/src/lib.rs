//! A herdr front for moor. Each open Box is a herdr workspace labelled `◈ repo/name` and
//! tagged with metadata tokens the user's sidebar config colours; its pane relays the agent's
//! terminal and turns into a card when the relay ends.

pub mod boxes;
pub mod card;
pub mod error;
pub mod herdr;
pub mod relay;
pub mod setup;
pub mod sync;
