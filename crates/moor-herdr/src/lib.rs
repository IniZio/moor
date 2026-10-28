//! A herdr front for moor. Each open Box is a herdr workspace labelled `◈ repo/name` and
//! tagged with metadata tokens the user's sidebar config colours; its pane relays the agent's
//! terminal and turns into a card when the relay ends.

pub mod auth;
pub mod boxes;
pub mod card;
pub mod connect;
pub mod error;
pub mod herdr;
pub mod panel;
pub mod picker;
pub mod relay;
pub mod setup;
pub mod shell;
pub mod sync;
