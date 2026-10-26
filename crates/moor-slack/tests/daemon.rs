//! Daemon tests: the `moor-slack` binary against a fake Slack and a scripted controller.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

#[path = "daemon/binding.rs"]
mod binding;
#[path = "daemon/close.rs"]
mod close;
#[path = "daemon/connect.rs"]
mod connect;
#[path = "daemon/feed.rs"]
mod feed;
#[path = "daemon/front.rs"]
mod front;
#[path = "daemon/identity.rs"]
mod identity;
#[path = "daemon/idle.rs"]
mod idle;
#[path = "daemon/inbox.rs"]
mod inbox;
#[path = "daemon/login.rs"]
mod login;
#[path = "daemon/mcp.rs"]
mod mcp;
#[path = "daemon/prompt.rs"]
mod prompt;
#[path = "daemon/proxy.rs"]
mod proxy;
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
#[path = "daemon/stop.rs"]
mod stop;
#[path = "daemon/sweep.rs"]
mod sweep;
#[path = "daemon/turn.rs"]
mod turn;
#[path = "daemon/unexpected_end.rs"]
mod unexpected_end;
