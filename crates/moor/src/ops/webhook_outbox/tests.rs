//! What a delivering front gets from `webhook-claim`, what `webhook-report` changes, and who
//! may send either.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test assertions index JSON values"
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::request::{WebhookClaimReq, WebhookOutcome, WebhookReportReq};
use moor_core::seam::clock::Ts;
use moor_core::user::{Caller, Identity, UserId};
use moor_core::view::user::WebhookClaimValue;
use moor_core::{BoxName, ErrorKind, Secret};
use serde_json::json;

use super::{claim, permit, report, FRONT_ONLY};
use crate::env::Env;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::vault::Vault;

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    alice: UserId,
    webhook: i64,
}

/// Alice's webhook on every `started` event, and one Box of hers that started.
fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let alice = store
        .enroll(&Identity::parse("local:2000").unwrap(), None)
        .unwrap()
        .0;
    let events = ["started".to_owned()];
    let key = Secret::new(b"the-key".to_vec());
    let webhook = store
        .add_user_webhook(&vault, alice, ("https://example.com/h", &events), &key, 10)
        .unwrap()
        .unwrap();
    let name = BoxName::parse_allow_bare("a").unwrap();
    store
        .apply(Decision::insert(NewBox { name, repo: None }).requester(alice))
        .unwrap();
    World {
        _dir: dir,
        store,
        vault,
        alice,
        webhook,
    }
}

fn claimed(w: &World) -> WebhookClaimValue {
    let req = WebhookClaimReq {
        limit: 10,
        lease_ms: 60_000,
    };
    serde_json::from_value(claim(&w.store, &w.vault, &req).unwrap()).unwrap()
}

#[test]
fn a_claim_carries_the_url_the_plain_key_and_a_moor_notify_body() {
    let w = world();
    let got = claimed(&w);
    let [one] = got.deliveries.as_slice() else {
        panic!("{got:?}")
    };
    assert_eq!(
        (one.webhook, one.url.as_str(), one.event.as_str()),
        (w.webhook, "https://example.com/h", "started")
    );
    assert_eq!(serde_json::to_value(&one.key).unwrap(), json!("the-key"));
    assert!(!format!("{one:?}").contains("the-key"));
    let body = &one.body;
    assert_eq!(body["schema"], "moor.notify/1");
    assert_eq!(body["delivery"], one.delivery);
    assert_eq!(
        (body["event"].as_str(), body["box"].as_str()),
        (Some("started"), Some("a"))
    );
    assert_eq!(body["ts"], 1_000_000);
    assert!(body["box_id"].is_i64() && body.get("data").is_some());
    assert!(
        body["lifecycle"].is_string() && body["session_state"].is_string(),
        "{body}"
    );
}

#[test]
fn a_body_names_the_offset_of_its_event_in_the_log() {
    let w = world();
    let got = claimed(&w);
    let [one] = got.deliveries.as_slice() else {
        panic!("{got:?}")
    };
    assert_eq!(one.body["offset"], w.store.last_event_id().unwrap());
}

#[test]
fn reports_fill_the_health_webhook_list_shows_and_refuse_an_unknown_delivery() {
    let w = world();
    let delivery = claimed(&w).deliveries[0].delivery;
    let failed = WebhookOutcome::Failed {
        reason: "HTTP 500".to_owned(),
    };
    report(
        &w.store,
        &WebhookReportReq {
            delivery,
            outcome: failed,
        },
    )
    .unwrap();
    let listed = w.store.user_webhooks(w.alice).unwrap().remove(0);
    assert_eq!(
        (listed.failing_since, listed.last_error.as_deref()),
        (Some(1_000_000), Some("HTTP 500"))
    );
    let again = WebhookReportReq {
        delivery,
        outcome: WebhookOutcome::Delivered,
    };
    let err = report(&w.store, &again).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn only_a_front_with_webhooks_true_may_claim_or_report() {
    let env = Env::default().with_front_webhooks(BTreeSet::from(["notify".to_owned()]));
    assert!(permit(
        |ns| env.front_webhooks(ns),
        &Ok(Caller::Front("notify".to_owned()))
    )
    .is_ok());
    for caller in [
        Ok(Caller::Front("web".to_owned())),
        Ok(Caller::Operator(1000)),
        Ok(Caller::Local(1001)),
        Ok(Caller::Runner),
        Err("no peer".to_owned()),
    ] {
        let err = permit(|ns| env.front_webhooks(ns), &caller).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused);
        assert!(
            err.error.to_string().contains("`webhooks: true`"),
            "{caller:?}"
        );
    }
    assert!(FRONT_ONLY.contains("webhooks: true"));
}

#[test]
fn the_wire_shapes_of_claim_and_report_are_stable() {
    let claim: moor_core::Request =
        serde_json::from_value(json!({"cmd": "webhook-claim", "limit": 5, "lease_ms": 30000}))
            .unwrap();
    assert_eq!(claim.cmd(), "webhook-claim");
    for outcome in [
        json!("delivered"),
        json!({"failed": {"reason": "HTTP 500"}}),
    ] {
        let req = json!({"cmd": "webhook-report", "delivery": 7, "outcome": outcome});
        let parsed: moor_core::Request = serde_json::from_value(req.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), req);
    }
}
