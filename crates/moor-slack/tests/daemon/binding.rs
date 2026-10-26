//! A thread's host Session is bound in the controller under front `slack` and the key
//! `<channel>/<thread ts>`, so a monitor can tell which conversation started a Box. A refused
//! bind is logged and the turn still runs.

use moor_core::request::Request;

use super::idle::{root_session, wait_posted};
use super::inbox::reply;
use super::support::controller::{refused, FakeController, Scripted};
use super::support::{Rig, CHANNEL, TS, USER};
use super::turn::{ask, GOOD};

/// `(front, key, session, owner)` of each `bind-session` the controller was sent.
fn binds(controller: &FakeController) -> Vec<(String, String, i64, String)> {
    controller.seen(|seen| {
        let bind = |r: &Request| match r {
            Request::BindSession(b) => Some((
                b.front.clone(),
                b.key.clone(),
                b.session.get(),
                b.owner.clone(),
            )),
            _ => None,
        };
        seen.requests.iter().filter_map(bind).collect()
    })
}

fn slack_binding(session: i64) -> (String, String, i64, String) {
    (
        "slack".into(),
        format!("{CHANNEL}/{TS}"),
        session,
        "slack".into(),
    )
}

#[test]
fn a_threads_first_turn_binds_its_host_session_under_the_slack_front() {
    let (rig, controller, _bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    assert_eq!(binds(&controller), [slack_binding(100)]);
    assert_eq!(controller.bindings(), [slack_binding(100)]);
    reply(&rig, 2, USER, "again");
    rig.slack.wait_posts(2);
    assert_eq!(binds(&controller).len(), 1, "a recorded Session binds once");
}

#[test]
fn a_new_host_session_after_a_clear_is_bound_again() {
    let (rig, controller, _bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    controller.end_host(100);
    reply(&rig, 2, USER, "and now?");
    rig.slack.wait_posts(2);
    assert_eq!(root_session(&rig), Some(101));
    assert_eq!(binds(&controller), [slack_binding(100), slack_binding(101)]);
}

#[test]
fn a_bind_the_controller_refuses_is_logged_and_the_turn_still_runs() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    let answer = Scripted::Reply(refused("bind-session", "no Session 100"));
    controller.script("bind-session", answer);
    let line = ask(&rig, "hello there");
    assert_eq!(line, "OK", "the turn ran");
    assert!(bin.join("run-0").exists(), "the agent ran");
    assert_eq!(root_session(&rig), Some(100));
    rig.wait_stderr(&format!(
        "could not bind host Session 100 as slack {CHANNEL}/{TS}: no Session 100"
    ));
}
