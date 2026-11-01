//! Over a front socket, `webhook-claim` and `webhook-report` answer only the front whose
//! config sets `webhooks: true`, and need no identity.
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
fn only_a_front_with_webhooks_true_claims_and_reports() {
    let dir = tempfile::tempdir().expect("dir");
    let cfg = config(
        dir.path(),
        "fronts:\n  notify:\n    webhooks: true\n  web: {}\n",
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
    let claim = json!({"cmd": "webhook-claim", "limit": 10, "lease_ms": 1000});
    let report = json!({"cmd": "webhook-report", "delivery": 1, "outcome": "delivered"});
    let got = ask(&socket("notify"), &claim);
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(got["value"], json!({"deliveries": []}));
    refused(
        &ask(&socket("notify"), &report),
        "no pending webhook delivery 1",
    );
    for request in [&claim, &report] {
        refused(&ask(&socket("web"), request), "`webhooks: true`");
    }
}
