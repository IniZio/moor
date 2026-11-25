//! The pull request switch on the workflow form's create and save, and a run's pull request
//! on the page.

use serde_json::{json, Value};

use super::super::login::{get, send};
use super::super::secrets::World;
use super::{as_user, csrf, json_of, world};

fn write(w: &World, cookie: &str, method: &str, path: &str, body: &Value) -> super::Reply {
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(w, cookie)
    );
    send(&w.addr, method, path, &extra, &body.to_string())
}

fn fields() -> Value {
    json!({"repos": ["acme/web"], "prompt": "p", "cron": "0 6 * * 1", "timezone": "UTC"})
}

/// The `workflow-add` requests the fake controller saw, oldest first.
fn adds(w: &World) -> Vec<Value> {
    let seen = w.seen.lock().unwrap().clone();
    seen.into_iter()
        .filter(|r| r["request"]["cmd"] == "workflow-add")
        .map(|r| r["request"].clone())
        .collect()
}

fn save(w: &World, cookie: &str, extra: &Value) -> super::Reply {
    let mut body = fields();
    body["revision"] = json!(3);
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    write(
        w,
        cookie,
        "PUT",
        "/account/api/workflows/weekly-deps/fields",
        &body,
    )
}

#[test]
fn a_create_sends_the_switch_as_set_and_off_when_it_is_not_sent() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-41");
    for (extra, want) in [
        (json!({"on_pull_request": true}), true),
        (json!({"on_pull_request": false}), false),
        (json!({}), false),
    ] {
        let mut body = fields();
        body["name"] = json!("fresh");
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        write(&w, &cookie, "POST", "/account/api/workflows", &body);
        let sent = adds(&w).pop().unwrap();
        assert_eq!(sent["name"], "fresh");
        assert_eq!(sent["on_pull_request"] == true, want, "{extra}");
    }
}

#[test]
fn a_save_turns_the_switch_on_and_off_and_keeps_it_when_it_is_not_sent() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-42");
    let on = save(&w, &cookie, &json!({"on_pull_request": true}));
    assert_eq!(on.status, 200, "{}", on.body);
    assert_eq!(json_of(&on)["on_pull_request"], true);
    assert_eq!(adds(&w).pop().unwrap()["on_pull_request"], true);

    let kept = save(&w, &cookie, &json!({"revision": 4}));
    assert_eq!(kept.status, 200, "{}", kept.body);
    assert_eq!(json_of(&kept)["on_pull_request"], true);
    assert_eq!(adds(&w).pop().unwrap()["on_pull_request"], true);

    let off = save(
        &w,
        &cookie,
        &json!({"revision": 5, "on_pull_request": false}),
    );
    assert_eq!(off.status, 200, "{}", off.body);
    assert_eq!(json_of(&off)["on_pull_request"], false);
    assert_ne!(adds(&w).pop().unwrap()["on_pull_request"], true);
}

#[test]
fn a_pull_request_run_names_its_pull_request_and_head_commit() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-pr");
    let flow = json_of(&get(
        &w.addr,
        "/account/api/workflows/weekly-deps",
        Some(&cookie),
    ));
    let runs = flow["runs"].as_array().unwrap();
    let pr = runs.iter().find(|r| r["id"] == 6).unwrap();
    assert_eq!(pr["pull"], "acme/web#5");
    assert_eq!(pr["head_sha"], "a41c9e2f00d");
}
