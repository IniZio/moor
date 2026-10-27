//! `machine-hold` over the controller socket: a Box claims one of its own user's machines, waits
//! for a slot or is refused, and lets go; a front socket may not send it.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::Decision;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName, HoldOp, HoldValue, MachineHoldReq, Request};
use serde_json::{json, Value};

use super::authz::{acting, A, B};
use super::fronts::{ask, refused, Served};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{box_repo, new_box_of};

const YAML: &str = "machine:\n  labels: [macos]\n  ports: [5432, 8080]\n";

/// A controller where user A owns machine `mini` of one slot and user B owns `studio`.
struct Hold {
    served: Served,
    store: Store,
}

impl Hold {
    fn new() -> Self {
        let served = Served::start();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let db = served.dir.path().join("state").join("moor.db");
        let store = Store::open(&db, clock).expect("store");
        let slack = served.front("slack");
        for (who, name, label) in [(A, "mini", "macos"), (B, "studio", "macos")] {
            assert_eq!(
                ask(&slack, &json!({"cmd": "enroll", "identity": who}))["class"],
                "ok"
            );
            let add =
                json!({"cmd": "machine-add", "name": name, "ssh": "dev@mac", "labels": [label]});
            assert_eq!(ask(&slack, &acting(who, &add))["class"], "ok");
        }
        Self { served, store }
    }

    /// A new open Box of `who`'s user whose repo asks for a macos machine.
    fn new_box(&self, who: &str, name: &str) -> BoxId {
        let identity = moor_core::user::Identity::parse(who).expect("identity");
        let user: UserId = self.store.user_of(&identity).expect("read").expect("user");
        let name = BoxName::parse_allow_bare(name).expect("name");
        let new: Decision = new_box_of(name, box_repo(Some(YAML))).requester(user);
        self.store.apply(new).expect("box").box_id
    }

    fn main(&self) -> &Path {
        &self.served.main
    }
}

fn hold(id: BoxId, op: &Value) -> Value {
    json!({"cmd": "machine-hold", "id": id.get(), "op": op})
}

fn claim() -> Value {
    json!({"claim": {}})
}

#[test]
fn a_claim_with_a_free_slot_holds_it_with_the_repos_ports() {
    let h = Hold::new();
    let a = h.new_box(A, "a");
    let got = ask(h.main(), &hold(a, &claim()));
    assert_eq!(
        got["value"],
        json!({"state": "held", "machine": "mini", "ssh": "dev@mac", "slot": 0,
            "ports": [5432, 8080]}),
        "{got}"
    );
    assert_eq!(
        ask(h.main(), &hold(a, &claim()))["value"],
        got["value"],
        "idempotent"
    );
}

#[test]
fn a_claim_on_a_full_machine_without_a_wait_is_refused_naming_the_holder() {
    let h = Hold::new();
    let (a, b) = (h.new_box(A, "a"), h.new_box(A, "b"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    let got = ask(h.main(), &hold(b, &claim()));
    assert_eq!(got["value"]["state"], "refused", "{got}");
    let reason = got["value"]["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("Box a"), "{got}");
}

#[test]
fn a_waiting_claim_is_held_once_the_holder_releases() {
    let h = Hold::new();
    let (a, b) = (h.new_box(A, "a"), h.new_box(A, "b"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    let wait = json!({"claim": {"wait_until": 4_000_000_000_000_i64}});
    let got = ask(h.main(), &hold(b, &wait));
    assert_eq!(got["value"]["state"], "waiting", "{got}");
    assert_eq!(got["value"]["position"], 1, "{got}");
    let released = ask(h.main(), &hold(a, &json!("release")));
    assert_eq!(
        released["value"],
        json!({"state": "released", "machine": "mini"}),
        "{released}"
    );
    let got = ask(h.main(), &hold(b, &wait));
    assert_eq!(got["value"]["state"], "held", "{got}");
    assert_eq!(got["value"]["slot"], 0, "{got}");
}

#[test]
fn a_release_frees_the_slot_for_another_box() {
    let h = Hold::new();
    let (a, b) = (h.new_box(A, "a"), h.new_box(A, "b"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    assert_eq!(
        ask(h.main(), &hold(a, &json!("release")))["value"]["state"],
        "released"
    );
    assert!(h.store.box_machine(a).expect("read").is_none());
    assert_eq!(ask(h.main(), &hold(b, &claim()))["value"]["state"], "held");
}

#[test]
fn naming_another_users_machine_is_refused() {
    let h = Hold::new();
    let a = h.new_box(A, "a");
    let got = ask(h.main(), &hold(a, &json!({"claim": {"machine": "studio"}})));
    assert_eq!(
        got["value"],
        json!({"state": "refused", "reason": "machine studio is not one of your machines"}),
        "{got}"
    );
}

#[test]
fn a_claim_by_labels_never_takes_another_users_machine() {
    let h = Hold::new();
    let (a, b) = (h.new_box(A, "a"), h.new_box(A, "b"));
    assert_eq!(
        ask(h.main(), &hold(a, &claim()))["value"]["machine"],
        "mini"
    );
    let got = ask(h.main(), &hold(b, &claim()));
    assert_eq!(got["value"]["state"], "refused", "{got}");
    assert!(h.store.box_machine(b).expect("read").is_none());
}

#[test]
fn a_shared_machine_gives_each_box_its_own_slot_and_ports_until_full() {
    let h = Hold::new();
    let add = json!({"cmd": "machine-add", "name": "mini", "ssh": "dev@mac",
        "labels": ["macos"], "capacity": 2});
    assert_eq!(
        ask(&h.served.front("slack"), &acting(A, &add))["class"],
        "ok"
    );
    let (a, b, c) = (h.new_box(A, "a"), h.new_box(A, "b"), h.new_box(A, "c"));
    let first = ask(h.main(), &hold(a, &claim()));
    assert_eq!(
        (&first["value"]["slot"], &first["value"]["ports"]),
        (&json!(0), &json!([5432, 8080]))
    );
    let second = ask(h.main(), &hold(b, &claim()));
    assert_eq!(
        (&second["value"]["slot"], &second["value"]["ports"]),
        (&json!(1), &json!([5442, 8090]))
    );
    let third = ask(h.main(), &hold(c, &claim()));
    assert_eq!(third["value"]["state"], "refused", "{third}");
    assert_eq!(
        ask(h.main(), &hold(a, &json!("release")))["value"]["state"],
        "released"
    );
    let again = ask(h.main(), &hold(c, &claim()));
    assert_eq!(
        (&again["value"]["slot"], &again["value"]["ports"]),
        (&json!(0), &json!([5432, 8080]))
    );
}

#[test]
fn a_front_may_not_send_machine_hold_even_for_its_own_box() {
    let h = Hold::new();
    let a = h.new_box(A, "a");
    let got = ask(&h.served.front("slack"), &acting(A, &hold(a, &claim())));
    refused(
        &got,
        "machine-hold is the machine runner's, which is operator-only",
    );
    assert!(h.store.box_machine(a).expect("read").is_none());
}

#[test]
fn a_request_and_its_values_round_trip_on_the_wire() {
    let req = Request::MachineHold(MachineHoldReq {
        id: BoxId::new(7),
        op: HoldOp::Claim {
            labels: vec!["macos".to_owned()],
            machine: Some("mini".to_owned()),
            ports: Some(vec![8080]),
            wait_until: Some(Ts::from_millis(5)),
        },
    });
    let wire = serde_json::to_value(&req).expect("encode");
    assert_eq!(
        wire,
        json!({"cmd": "machine-hold", "id": 7, "op": {"claim": {"labels": ["macos"],
            "machine": "mini", "ports": [8080], "wait_until": 5}}})
    );
    assert_eq!(req.cmd(), "machine-hold");
    assert_eq!(
        serde_json::from_value::<Request>(wire).expect("decode"),
        req
    );
    let release: Request =
        serde_json::from_value(json!({"cmd": "machine-hold", "id": 7, "op": "release"}))
            .expect("decode");
    assert_eq!(
        release,
        Request::MachineHold(MachineHoldReq {
            id: BoxId::new(7),
            op: HoldOp::Release
        })
    );
    for value in [
        HoldValue::Held {
            machine: "mini".to_owned(),
            ssh: "dev@mac".to_owned(),
            slot: 1,
            ports: vec![8090],
        },
        HoldValue::Waiting {
            position: 2,
            reason: "full".to_owned(),
        },
        HoldValue::Refused {
            reason: "no".to_owned(),
        },
        HoldValue::Released { machine: None },
    ] {
        let wire = serde_json::to_value(&value).expect("encode");
        assert_eq!(
            serde_json::from_value::<HoldValue>(wire).expect("decode"),
            value
        );
    }
}

impl Hold {
    /// The machine events recorded for `id`, as (kind, payload), oldest first.
    fn machine_events(&self, id: BoxId) -> Vec<(String, Value)> {
        let rows = self.store.events(id, 100).expect("events");
        rows.into_iter()
            .filter(|e| e.kind.starts_with("machine_"))
            .map(|e| (e.kind, e.payload))
            .collect()
    }
}

fn wait() -> Value {
    json!({"claim": {"wait_until": 4_000_000_000_000_i64}})
}

#[test]
fn polling_a_wait_at_the_same_place_records_one_event_and_a_new_place_one_more() {
    let h = Hold::new();
    let (a, b, c) = (h.new_box(A, "a"), h.new_box(A, "b"), h.new_box(A, "c"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    for _ in 0..3 {
        let got = ask(h.main(), &hold(b, &wait()));
        assert_eq!(got["value"]["position"], 1, "{got}");
    }
    let got = ask(h.main(), &hold(c, &wait()));
    assert_eq!(got["value"]["position"], 2, "{got}");
    let events = h.machine_events(b);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "machine_waiting");
    assert_eq!(events[0].1["position"], 1);
    assert_eq!(events[0].1["name"], "macos");
    assert!(events[0].1["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("Box a"));
}

#[test]
fn a_want_that_moves_up_the_line_records_its_new_place_once() {
    let h = Hold::new();
    let (a, b, c) = (h.new_box(A, "a"), h.new_box(A, "b"), h.new_box(A, "c"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    ask(h.main(), &hold(b, &wait()));
    for _ in 0..2 {
        assert_eq!(ask(h.main(), &hold(c, &wait()))["value"]["position"], 2);
    }
    ask(h.main(), &hold(b, &json!("release")));
    for _ in 0..2 {
        assert_eq!(ask(h.main(), &hold(c, &wait()))["value"]["position"], 1);
    }
    let places: Vec<_> = h
        .machine_events(c)
        .iter()
        .map(|e| e.1["position"].clone())
        .collect();
    assert_eq!(places, [json!(2), json!(1)]);
}

#[test]
fn a_refused_claim_records_one_refused_event_and_keeps_no_want() {
    let h = Hold::new();
    let (a, b) = (h.new_box(A, "a"), h.new_box(A, "b"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    let got = ask(h.main(), &hold(b, &claim()));
    assert_eq!(got["value"]["state"], "refused", "{got}");
    let events = h.machine_events(b);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "machine_refused");
    assert_eq!(events[0].1["reason"], got["value"]["reason"]);
    assert!(h.store.wait_place(b).expect("read").is_none());
}

#[test]
fn show_reports_a_waiting_claim_with_its_place_in_line() {
    let h = Hold::new();
    let (a, b, c) = (h.new_box(A, "a"), h.new_box(A, "b"), h.new_box(A, "c"));
    assert_eq!(ask(h.main(), &hold(a, &claim()))["value"]["state"], "held");
    ask(h.main(), &hold(b, &wait()));
    ask(h.main(), &hold(c, &wait()));
    let got = ask(h.main(), &json!({"cmd": "show", "box": "c"}));
    let held = &got["value"]["box"]["held_machine"];
    assert_eq!(held["state"], "waiting", "{got}");
    assert_eq!(held["position"], 2, "{got}");
    assert_eq!(held["name"], "macos", "{got}");
}
