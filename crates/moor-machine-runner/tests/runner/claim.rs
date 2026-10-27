use serde_json::{json, Value};

use crate::rig::{Rig, HEADERS};

const MISSING: [(&str, &str); 2] = [
    ("X-Moor-Box", "7"),
    ("X-Moor-Machine-Missing", "this Box holds no machine"),
];

fn waiting() -> Value {
    json!({"state": "waiting", "position": 1, "reason": "machine minion is attached to Boxes 3"})
}

fn held() -> Value {
    json!({"state": "held", "machine": "minion", "ssh": "moor@minion", "slot": 0,
        "ports": [8081]})
}

/// A rig whose fake controller answers claims with `answers`, in turn.
fn answering(args: &[&str], answers: Vec<Value>) -> Rig {
    let rig = Rig::with_args(args);
    rig.holds.lock().unwrap().answers = answers.into();
    rig
}

fn seen(rig: &Rig) -> Vec<Value> {
    rig.holds.lock().unwrap().seen.clone()
}

#[test]
fn a_claim_with_a_free_slot_returns_the_slot_and_the_next_exec_runs() {
    let rig = answering(&[], vec![held()]);
    let got = rig.call_with("machine_claim", json!({}), &MISSING).unwrap();
    assert_eq!(
        got,
        json!({"machine": "minion", "slot": 0, "port_offset": 0, "ports": [8081], "facts": {}})
    );
    assert_eq!(
        seen(&rig),
        [json!({"op": {"claim": {}}, "suspended": false})]
    );
    std::fs::create_dir_all(rig.copy()).unwrap();
    let out = rig.call("exec", json!({"command": "echo ran"})).unwrap();
    assert_eq!(out["exit_code"], 0, "{out}");
}

#[test]
fn a_claim_on_a_full_machine_without_waiting_is_refused_naming_the_holders() {
    let refused = json!({"state": "refused", "reason": "machine minion is attached to Boxes 3, 4"});
    let rig = answering(&[], vec![refused]);
    let err = rig
        .call_with("machine_claim", json!({"labels": ["mac"]}), &MISSING)
        .unwrap_err();
    assert_eq!(
        err,
        "no free slot: machine minion is attached to Boxes 3, 4"
    );
    assert_eq!(seen(&rig).len(), 1);
}

#[test]
fn a_waiting_claim_reports_its_place_until_it_is_held() {
    let rig = answering(&[], vec![waiting(), waiting(), held()]);
    let args = json!({"machine": "minion", "wait_secs": 60});
    let (got, progress) = rig.call_progress("machine_claim", args, &MISSING);
    assert_eq!(got.unwrap()["slot"], 0);
    assert!(
        progress
            .iter()
            .any(|p| p == "waiting for minion: place 1 in line"),
        "{progress:?}"
    );
    let seen = seen(&rig);
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen[0]["op"]["claim"]["wait_until"].is_i64(), "{seen:?}");
    assert_eq!(seen[0]["op"]["claim"]["machine"], "minion");
}

#[test]
fn a_wait_that_runs_out_is_refused_and_releases_the_want() {
    let rig = answering(&[], vec![waiting()]);
    let err = rig
        .call_with("machine_claim", json!({"wait_secs": 1}), &MISSING)
        .unwrap_err();
    assert_eq!(err, "no free slot: machine minion is attached to Boxes 3");
    assert_eq!(seen(&rig).last().unwrap()["op"], "release");
}

#[test]
fn a_wait_over_an_hour_is_refused_with_the_reason() {
    let rig = answering(&[], vec![held()]);
    let err = rig
        .call_with("machine_claim", json!({"wait_secs": 3601}), &MISSING)
        .unwrap_err();
    assert_eq!(err, "wait_secs must be at most 3600, not 3601");
    assert!(seen(&rig).is_empty());
}

#[test]
fn release_suspends_the_box_first_and_a_new_claim_does_not_sync() {
    let suspend = "echo s >> ~/suspended";
    let rig = answering(
        &["--prepare", "echo '{}'", "--suspend", suspend],
        vec![held()],
    );
    rig.call_with("machine_claim", json!({}), &MISSING).unwrap();
    rig.call("sync", json!({})).unwrap();
    let got = rig.call("machine_release", json!({})).unwrap();
    assert_eq!(got, json!({"released": "minion"}));
    let release = seen(&rig).last().cloned().unwrap();
    assert_eq!(release, json!({"op": "release", "suspended": true}));
    rig.call_with("machine_claim", json!({}), &MISSING).unwrap();
    assert_eq!(rig.mirror_excludes.lock().unwrap().len(), 1);
}

#[test]
fn a_held_slot_on_a_machine_too_full_to_use_is_released_and_the_claim_fails() {
    let rig = answering(&[], vec![held()]);
    rig.set_free_kb(1_048_576);
    let err = rig
        .call_with("machine_claim", json!({}), &MISSING)
        .unwrap_err();
    assert!(err.contains("minion") && err.contains("released"), "{err}");
    let ops: Vec<Value> = seen(&rig).into_iter().map(|s| s["op"].clone()).collect();
    assert_eq!(ops, [json!({"claim": {}}), json!("release")]);
}

#[test]
fn a_held_claim_tells_the_client_the_tool_list_changed() {
    let rig = answering(&[], vec![held()]);
    let (got, methods) = rig.call_watching("machine_claim", json!({}), &MISSING);
    got.unwrap();
    assert!(
        methods
            .iter()
            .any(|m| m == "notifications/tools/list_changed"),
        "{methods:?}"
    );
}

#[test]
fn both_tools_answer_while_the_box_holds_no_machine_and_others_stay_refused() {
    let rig = answering(&[], vec![held()]);
    let got = rig
        .call_with("machine_release", json!({}), &MISSING)
        .unwrap();
    assert_eq!(got, json!({"released": null}));
    let err = rig
        .call_with("machine_info", json!({}), &MISSING)
        .unwrap_err();
    assert_eq!(err, "this Box holds no machine. Try again later.");
    rig.call_with("machine_claim", json!({}), &MISSING).unwrap();
    assert_eq!(seen(&rig)[0]["op"], "release");
}

#[test]
fn every_tool_is_listed_to_a_box_with_a_machine() {
    let rig = Rig::start();
    let names: Vec<_> = rig
        .tools(&HEADERS)
        .iter()
        .map(|t| t["name"].clone())
        .collect();
    assert!(names.contains(&json!("machine_claim")), "{names:?}");
}
