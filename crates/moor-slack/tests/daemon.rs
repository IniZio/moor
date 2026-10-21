//! Daemon tests: the `moor-slack` binary against a fake Slack and a scripted controller.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

#[path = "daemon/close.rs"]
mod close;
#[path = "daemon/connect.rs"]
mod connect;
#[path = "daemon/idle.rs"]
mod idle;
#[path = "daemon/inbox.rs"]
mod inbox;
#[path = "daemon/login.rs"]
mod login;
#[path = "daemon/mcp.rs"]
mod mcp;
#[path = "daemon/repo.rs"]
mod repo;
#[path = "daemon/restart.rs"]
mod restart;
#[path = "daemon/smoke.rs"]
mod smoke;
#[path = "daemon/start.rs"]
mod start;
#[path = "daemon/state.rs"]
mod state;
#[path = "daemon/sweep.rs"]
mod sweep;
#[path = "daemon/turn.rs"]
mod turn;
