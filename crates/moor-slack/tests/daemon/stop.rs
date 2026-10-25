//! The Stop button on a Box's status message: its requester presses it to interrupt the
//! agent; anyone else is refused; it goes away when the Box closes.

#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::request::Request;
use serde_json::{json, Value};

use super::inbox::OTHER;
use super::support::controller::FakeController;
use super::support::slack_fake::Click;
use super::support::{Rig, BOX_NAME, CHANNEL, QUIET, TS, USER};

/// The id the scripted controller gives the first Box it starts.
const BOX_ID: i64 = 1;

/// Whether `blocks` holds a Stop button.
fn has_stop(blocks: &Value) -> bool {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "actions")
        .flat_map(|b| b["elements"].as_array().cloned().unwrap_or_default())
        .any(|e| e["action_id"] == "stop_box" && e["value"] == BOX_NAME)
}

/// A thread whose Box is working; returns the ts of its status message once it shows Stop.
fn working() -> (Rig, FakeController, std::path::PathBuf, String) {
    let (rig, controller, bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    controller.emit(BOX_ID, BOX_NAME, "working", json!({}));
    let (posts, updates) = rig
        .slack
        .wait_until("a status message with Stop", |posts, updates| {
            posts.iter().any(|p| has_stop(&p.blocks)) || updates.iter().any(|u| has_stop(&u.blocks))
        });
    let post = posts.iter().find(|p| has_stop(&p.blocks));
    let ts = post.map_or_else(
        || {
            updates
                .iter()
                .find(|u| has_stop(&u.blocks))
                .unwrap()
                .ts
                .clone()
        },
        |p| p.ts.clone(),
    );
    (rig, controller, bin, ts)
}

/// Presses Stop on status message `ts` as `user`.
fn stop(rig: &Rig, n: u32, user: &str, ts: &str) {
    rig.slack.send_click(&Click {
        envelope_id: &format!("env-stop-{n}"),
        channel: CHANNEL,
        user,
        message_ts: ts,
        thread_ts: TS,
        value: BOX_NAME,
        action_id: "stop_box",
    });
}

/// The Box of each `steer` the controller got that interrupts.
fn interrupts(controller: &FakeController) -> Vec<String> {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter_map(|r| match r {
                Request::Steer(s) if s.interrupt => Some(s.target.to_string()),
                _ => None,
            })
            .collect()
    })
}

#[test]
fn the_requester_pressing_stop_interrupts_the_agent() {
    let (rig, controller, _bin, ts) = working();
    stop(&rig, 1, USER, &ts);
    controller.wait_until("the interrupt", |seen| {
        seen.requests
            .iter()
            .any(|r| matches!(r, Request::Steer(s) if s.interrupt))
    });
    assert_eq!(interrupts(&controller), [BOX_NAME.to_owned()]);
    assert!(rig.slack.ephemerals().is_empty());
}

#[test]
fn anyone_else_pressing_stop_gets_an_ephemeral_refusal_and_nothing_is_interrupted() {
    let (rig, controller, _bin, ts) = working();
    stop(&rig, 1, OTHER, &ts);
    let ephemerals = rig.slack.wait_ephemerals(1);
    assert_eq!(ephemerals[0].user, OTHER);
    assert_eq!(
        ephemerals[0].text,
        format!("Only <@{USER}> can stop this Box.")
    );
    std::thread::sleep(QUIET);
    assert!(interrupts(&controller).is_empty());
}

#[test]
fn the_button_goes_away_when_the_box_closes() {
    let (rig, controller, _bin, ts) = working();
    controller.emit(BOX_ID, BOX_NAME, "closed", json!({}));
    let (_, updates) = rig
        .slack
        .wait_until("the status without Stop", |_, updates| {
            updates.iter().any(|u| u.ts == ts && u.blocks == json!([]))
        });
    let last = updates.iter().rev().find(|u| u.ts == ts).unwrap();
    assert_eq!(last.blocks, json!([]));
    assert!(last.text.ends_with("closed"), "{}", last.text);
}
