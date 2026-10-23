//! A turn reaches Claude only through the front Proxy: a Proxy that is down when the turn
//! starts, or found down when the turn fails, is named in the thread instead of a generic
//! failure, and a failure while it runs keeps its own line.

use moor_core::response::Response;
use serde_json::json;

use super::support::controller::Scripted;
use super::support::Rig;
use super::turn::{ask, turns, GOOD};

/// What the thread is shown when the Proxy is down.
const PROXY_DOWN: &str = "The coordinator is unavailable: the Proxy that carries Claude's requests is not running, and moor is restarting it. Send your message again in a moment.";

/// A lease the fake answers with an error that names it.
const FAILING: &str = "lease-echo-2b9f";

#[test]
fn a_proxy_down_at_the_start_is_named_and_nothing_runs() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    controller.proxy_down();
    assert_eq!(ask(&rig, "hello"), PROXY_DOWN);
    assert!(!bin.join("run-0").exists(), "the agent ran");
    assert_eq!(turns(&rig).first().map(|t| t.0.as_str()), Some("failed"));
}

#[test]
fn a_turn_that_fails_while_the_proxy_is_down_names_the_proxy() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(FAILING));
    let down = Response::ok("proxy-status", Some(json!({"running": false})));
    controller.script("proxy-status", Scripted::Reply(down));
    assert_eq!(ask(&rig, "hello"), PROXY_DOWN);
    assert!(bin.join("run-0").exists(), "the agent ran");
}

#[test]
fn a_turn_that_fails_while_the_proxy_runs_keeps_its_own_line() {
    let (rig, _controller, _bin) = Rig::start_coordinated(Some(FAILING));
    assert_eq!(
        ask(&rig, "hello"),
        "The coordinator stopped: Bad token [token]."
    );
}
