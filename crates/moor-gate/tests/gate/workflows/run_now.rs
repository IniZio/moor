//! Run now on the workflows routes: a workflow fired by its owner.

use moor_core::response::Response;
use serde_json::{json, Value};

use super::super::login::send;
use super::super::secrets::World;
use super::{answer_run, as_user, csrf, json_of, world};

/// What the fake controller answers a run now: a refusal while the workflow's run waits, else a
/// started run.
pub(super) fn reply(open: bool) -> Response {
    let value = if open {
        json!({"workflow": "weekly-deps", "status": "refused",
            "reason": "a run of weekly-deps is still open (waiting, Box weekly-deps-1)"})
    } else {
        json!({"workflow": "weekly-deps", "status": "started",
            "box": "weekly-deps/w1-m20260101000000"})
    };
    Response::ok("x", Some(value))
}

fn run_now(w: &World, cookie: &str, token: &str, body: &str) -> super::Reply {
    let extra =
        format!("Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {token}\r\n");
    send(
        &w.addr,
        "POST",
        "/account/api/workflows/weekly-deps/run",
        &extra,
        body,
    )
}

#[test]
fn run_now_asks_as_the_session_person_and_a_started_run_is_ok() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-31");
    assert_eq!(answer_run(&w, &cookie, "yes", true).status, 200);
    let body = "{}";
    let reply = run_now(&w, &cookie, &csrf(&w, &cookie), body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply)["status"], "started");
    let seen = w.seen.lock().unwrap().clone();
    let sent = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "workflow-run-now")
        .unwrap();
    assert_eq!(sent["who"], "authgear:alice-31");
    assert_eq!(sent["request"]["name"], "weekly-deps");
    assert_eq!(sent["request"].get("repos"), None);
    assert!(sent["request"].get("identity").is_none_or(Value::is_null));
}

#[test]
fn a_workflow_with_an_open_run_is_a_conflict_that_says_why() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-32");
    let body = "{}";
    let reply = run_now(&w, &cookie, &csrf(&w, &cookie), body);
    assert_eq!(reply.status, 409, "{}", reply.body);
    let error = json_of(&reply)["error"].as_str().unwrap().to_owned();
    assert!(error.contains("still open"), "{error}");
}

#[test]
fn a_forged_page_or_a_stranger_send_nothing() {
    let w = world(true, &[]);
    let alice = as_user(&w, "alice-33");
    let body = "{}";
    assert_eq!(run_now(&w, &alice, "forged", body).status, 403);
    assert!(!w.cmds().contains(&"workflow-run-now".to_owned()));
    let none = send(
        &w.addr,
        "POST",
        "/account/api/workflows/weekly-deps/run",
        "",
        body,
    );
    assert_eq!(none.status, 401);
    let bob = as_user(&w, "bob-33");
    let reply = run_now(&w, &bob, &csrf(&w, &bob), body);
    assert_eq!(reply.status, 404, "{}", reply.body);
}

#[test]
fn the_runs_list_shows_what_fired_each_run() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-34");
    let reply = super::get(&w.addr, "/account/api/workflows/weekly-deps", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply)["runs"][0]["trigger"], "cron");
}
