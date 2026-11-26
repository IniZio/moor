//! A run read by its id alone: any signed-in person may, and only its owner is told it is theirs.

use moor_core::response::{ErrorKind, Response};
use serde_json::{json, Value};

use super::{as_user, delivered_run, fresh, get, json_of, refusal, waiting_run, world, OWNER};

/// What the fake controller answers `workflow-run-read`: any person reads run 6 or 7.
pub(super) fn reply(who: &str, inner: &Value) -> Response {
    let held = fresh();
    let mut run = match inner["run"].as_i64() {
        Some(7) => waiting_run(&held),
        Some(6) => delivered_run(),
        _ => return refusal(ErrorKind::Refused, "no run 99"),
    };
    run["plan"] = held.plan.unwrap_or(Value::Null);
    let yours = who.contains(OWNER);
    Response::ok(
        "x",
        Some(json!({"workflow": "weekly-deps", "yours": yours, "run": run})),
    )
}

#[test]
fn any_signed_in_person_reads_a_run_by_its_id_and_is_told_whether_it_is_theirs() {
    let w = world(true, &[]);
    let bob = as_user(&w, "bob-21");
    let reply = get(&w.addr, "/account/api/runs/7", Some(&bob));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let run = json_of(&reply);
    assert_eq!(run["workflow"], "weekly-deps");
    assert_eq!(run["yours"], false);
    assert_eq!(run["id"], 7);
    assert_eq!(run["steps"].as_array().unwrap().len(), 4);
    let alice = as_user(&w, "alice-21");
    let mine = json_of(&get(&w.addr, "/account/api/runs/7", Some(&alice)));
    assert_eq!(mine["yours"], true);
    assert_eq!(
        get(&w.addr, "/account/api/runs/99", Some(&alice)).status,
        404
    );
    assert_eq!(get(&w.addr, "/account/api/runs/7", None).status, 401);
}
