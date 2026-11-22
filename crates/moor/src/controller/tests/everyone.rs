//! Any signed-in user reads every user's Boxes through a read that asks for everyone's, and
//! acts on none of them: every write stays the owner's.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use serde_json::{json, Value};

use super::authz::{acting, every_kind, naming, not_yours, Names, World, A, A_BOX, B, B_BOX};
use super::fronts::ask;

const NO_USER_BOX: &str = "fixture-box";
const OPERATOR_BOX: &str = "op-box";

/// The kinds that read a Box and may ask for everyone's.
const READS: [&str; 3] = ["show", "artifacts", "artifact-read"];

/// The names and owners of the Boxes a `show` with no Box lists.
fn listed(envelope: &Value) -> Vec<(String, Value)> {
    let boxes = envelope["value"]["boxes"]
        .as_array()
        .expect("a list of Boxes");
    let mut rows: Vec<(String, Value)> = boxes
        .iter()
        .map(|b| {
            (
                b["name"].as_str().expect("name").to_owned(),
                b["owner"].clone(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// `request` with `"everyone": true` set over it.
fn everyone(mut request: Value) -> Value {
    request["everyone"] = json!(true);
    request
}

#[test]
fn a_user_lists_every_users_box_with_its_owner_and_no_box_without_a_user() {
    let world = World::new();
    let got = ask(
        &world.herdr(),
        &acting(A, &everyone(json!({"cmd": "show"}))),
    );
    assert_eq!(got["class"], "ok", "{got}");
    let rows = listed(&got);
    let names: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, [A_BOX, B_BOX, OPERATOR_BOX], "{got}");
    let owner = |name: &str| rows.iter().find(|(n, _)| n == name).expect(name).1.clone();
    assert_eq!(owner(A_BOX), Value::Null, "a user's own Box names no owner");
    assert!(owner(B_BOX).is_string(), "{got}");
    assert_eq!(owner(OPERATOR_BOX), "operator@example.com");
}

#[test]
fn without_everyone_a_list_is_still_the_callers_own() {
    let world = World::new();
    let got = ask(&world.herdr(), &acting(A, &json!({"cmd": "show"})));
    let names: Vec<String> = listed(&got).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, [A_BOX], "{got}");
}

#[test]
fn a_user_views_another_users_box_and_its_artifacts_with_everyone() {
    let world = World::new();
    let herdr = world.herdr();
    let got = ask(
        &herdr,
        &acting(
            A,
            &everyone(json!({"cmd": "show", "box": B_BOX, "events": 10})),
        ),
    );
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(got["value"]["box"]["name"], B_BOX, "{got}");
    assert!(got["value"]["box"]["owner"].is_string(), "{got}");
    let listed = ask(
        &herdr,
        &acting(A, &everyone(json!({"cmd": "artifacts", "box": B_BOX}))),
    );
    assert_eq!(listed["class"], "ok", "{listed}");
    let read =
        everyone(json!({"cmd": "artifact-read", "box": B_BOX, "name": "n", "offset": 0, "len": 1}));
    let got = ask(&herdr, &acting(A, &read));
    assert_ne!(got["error"]["message"], "not your Box", "{got}");
}

#[test]
fn everyone_reaches_no_pane_no_box_without_a_user_and_no_unknown_identity() {
    let world = World::new();
    let herdr = world.herdr();
    let pane = everyone(json!({"cmd": "show", "box": B_BOX, "pane": true}));
    not_yours(&ask(&herdr, &acting(A, &pane)));
    for user in [A, B] {
        let show = everyone(json!({"cmd": "show", "box": NO_USER_BOX}));
        not_yours(&ask(&herdr, &acting(user, &show)));
    }
    let stranger = "herdr:T1:Z";
    not_yours(&ask(
        &herdr,
        &acting(stranger, &everyone(json!({"cmd": "show", "box": B_BOX}))),
    ));
    let list = ask(&herdr, &acting(stranger, &everyone(json!({"cmd": "show"}))));
    assert!(listed(&list).is_empty(), "{list}");
}

#[test]
fn every_write_on_another_users_box_is_refused_with_or_without_everyone() {
    let world = World::new();
    let herdr = world.herdr();
    let mut checked = 0;
    for kind in every_kind().iter().filter(|k| !READS.contains(&k.as_str())) {
        let Names::Box(write) = naming(kind, B_BOX, world.b_session) else {
            continue;
        };
        not_yours(&ask(&herdr, &acting(A, &write)));
        let got = ask(&herdr, &acting(A, &everyone(write)));
        assert_ne!(got["class"], "ok", "{kind}: {got}");
        checked += 1;
    }
    assert_eq!(checked, 28);
    let b = world
        .store
        .box_row(world.b_box)
        .expect("read")
        .expect("row");
    assert!(b.lifecycle.is_live(), "B's Box stays open");
}

#[test]
fn a_read_without_everyone_on_another_users_box_is_still_refused() {
    let world = World::new();
    let herdr = world.herdr();
    let mut checked = 0;
    for kind in READS {
        let Names::Box(read) = naming(kind, B_BOX, world.b_session) else {
            continue;
        };
        not_yours(&ask(&herdr, &acting(A, &read)));
        checked += 1;
    }
    assert_eq!(checked, READS.len());
}
