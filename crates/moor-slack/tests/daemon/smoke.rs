//! Smoke tests against a real `moor controller serve` with no repos, reached only through
//! its socket. The fake `claude` calls `start_box` in every turn.

#![allow(clippy::indexing_slicing, reason = "test code")]

use serde_json::json;

use super::support::{Rig, BOX_NAME, MCP, TS, USER};

/// The refusal the controller gives for any repo when none are configured.
const UNKNOWN: &str = "repo nosuch is not known to the controller; no repos are configured";

/// A daemon on a real controller whose channel's default repo is `nosuch`.
fn rig_on_a_real_controller() -> (Rig, super::support::RealController) {
    let mut rig = Rig::new();
    let bin = rig.use_fake_claude(None);
    rig.link_token(USER, MCP);
    Rig::tool_calls(
        &bin,
        None,
        &[("start_box", json!({"goal": "fix the thing"}))],
    );
    let controller = rig.real_controller();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.set_default_repo("nosuch");
    (rig, controller)
}

#[test]
fn real_controller_refusal_is_posted_in_the_thread() {
    let (rig, _controller) = rig_on_a_real_controller();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}"));
}

/// The controller stops after the first thread's start and a new one takes its socket; the
/// second thread opens a host Session on it and gets its refusal.
#[test]
fn real_controller_restart_is_survived() {
    let (rig, first) = rig_on_a_real_controller();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.wait_post(&format!("Could not start Box `{BOX_NAME}`: {UNKNOWN}"));
    drop(first);
    let _second = rig.real_controller();
    let ts = "1730000009.000300";
    rig.mention(2, USER, "<@UBOT> fix the thing", ts, None);
    rig.wait_post(&format!(
        "Could not start Box `slack-1730000009-000300-1`: {UNKNOWN}"
    ));
}
