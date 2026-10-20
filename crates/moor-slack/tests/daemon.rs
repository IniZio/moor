//! Daemon tests: the `moor-slack` binary against a fake Slack and a scripted controller.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

#[path = "daemon/connect.rs"]
mod connect;
#[path = "daemon/smoke.rs"]
mod smoke;
#[path = "daemon/start.rs"]
mod start;
#[path = "daemon/state.rs"]
mod state;
