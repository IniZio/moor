//! Over a front socket, `step-report` answers only the front whose config sets
//! `step_reports: true`, and needs no identity: the front names the Box.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::fronts::{ask, refused};
use super::{config, env_with, Buf};
use crate::controller::{start, Sink};

#[test]
fn only_a_front_with_step_reports_true_reports_a_step() {
    let dir = tempfile::tempdir().expect("dir");
    let cfg = config(
        dir.path(),
        "fronts:\n  gate:\n    step_reports: true\n  web: {}\n",
    );
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    let socket = |ns: &str| {
        dir.path()
            .join("state")
            .join("fronts")
            .join(ns)
            .join("front")
    };
    let report = json!({
        "cmd": "step-report",
        "box_id": 7,
        "report": {"step": "audit", "result": "ok", "evidence": [], "note": ""}
    });
    let got = ask(&socket("gate"), &report);
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(got["value"]["error"], true, "{got}");
    assert!(
        got["value"]["reply"]
            .as_str()
            .is_some_and(|r| r.contains("no running workflow run")),
        "{got}"
    );
    refused(&ask(&socket("web"), &report), "`step_reports: true`");
}

#[test]
fn only_a_front_with_step_reports_true_reports_an_output() {
    let dir = tempfile::tempdir().expect("dir");
    let cfg = config(
        dir.path(),
        "fronts:\n  gate:\n    step_reports: true\n  web: {}\n",
    );
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    let socket = |ns: &str| {
        dir.path()
            .join("state")
            .join("fronts")
            .join(ns)
            .join("front")
    };
    let report = json!({
        "cmd": "output-report", "box_id": 7, "kind": "github.comment",
        "url": "https://example.com/acme/web/pull/5#c1", "title": "Evidence"
    });
    refused(&ask(&socket("gate"), &report), "no Box 7");
    refused(&ask(&socket("web"), &report), "`step_reports: true`");
}

#[test]
fn only_a_front_with_step_reports_true_hands_in_a_reading() {
    let dir = tempfile::tempdir().expect("dir");
    let cfg = config(
        dir.path(),
        "fronts:\n  gate:\n    step_reports: true\n  web: {}\n",
    );
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    let socket = |ns: &str| {
        dir.path()
            .join("state")
            .join("fronts")
            .join(ns)
            .join("front")
    };
    let hand_in = json!({
        "cmd": "reading-hand-in", "box_id": 7,
        "reading": {"phrases": [], "effects": []}
    });
    refused(&ask(&socket("gate"), &hand_in), "no Box 7");
    refused(&ask(&socket("web"), &hand_in), "`step_reports: true`");
}
