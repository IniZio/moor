//! The machine settings route under `/account/api/nodes` and the node settings tools, against
//! the testkit issuer and a fake controller that keeps one personal node's settings in memory.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::login::{get, send, sign_in, Reply};
use super::secrets::{world, World};

/// The settings of the node `hv`: its cap and labels, its revision and who wrote it.
pub(super) struct Settings {
    max_boxes: Option<u32>,
    labels: Vec<String>,
    revision: u64,
    edited_by: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_boxes: Some(4),
            labels: Vec::new(),
            revision: 0,
            edited_by: None,
        }
    }
}

fn refusal(kind: ErrorKind, message: &str) -> Response {
    let view = ErrorView {
        kind,
        message: message.to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    Response::err("x", view)
}

/// What `node-settings-get` answers.
fn held(s: &Settings) -> Value {
    json!({"name": "hv", "scope": "personal", "max_boxes": s.max_boxes, "labels": s.labels,
        "revision": s.revision, "edited_by": s.edited_by,
        "edited_at": s.edited_by.as_ref().map(|_| "2026-11-17T10:00:00Z"), "can_edit": true})
}

/// The answer to `node-list`: the personal node `hv`, online.
pub(super) fn list(s: &Settings) -> Value {
    json!({"nodes": [{"name": "hv", "scope": "personal", "runtime": "cloud-hypervisor",
        "online": true, "used": 1, "waiting": 0, "max_boxes": s.max_boxes, "labels": s.labels,
        "revision": s.revision, "editable": true}], "at": 100})
}

/// What the fake controller answers `node-settings-get` and `node-settings-set` from `who`.
pub(super) fn answer(s: &mut Settings, who: &str, inner: &Value) -> Response {
    if inner["name"] != "hv" {
        return refusal(ErrorKind::Refused, "you have no node named that");
    }
    if inner["cmd"] == "node-settings-get" {
        return Response::ok("x", Some(held(s)));
    }
    if inner["expected_revision"]
        .as_u64()
        .is_some_and(|r| r != s.revision)
    {
        return refusal(ErrorKind::Refused, "they changed after you read them");
    }
    match &inner["max_boxes"] {
        Value::Null => {}
        Value::String(_) => s.max_boxes = None,
        set => s.max_boxes = set["set"].as_u64().and_then(|n| u32::try_from(n).ok()),
    }
    if let Some(labels) = inner["labels"].as_array() {
        if labels.iter().any(|l| l == "bogus") {
            return refusal(ErrorKind::Usage, "label `bogus` is not accepted");
        }
        s.labels = labels
            .iter()
            .filter_map(|l| l.as_str().map(str::to_owned))
            .collect();
    }
    s.revision += 1;
    s.edited_by = Some(who.to_owned());
    Response::ok("x", Some(held(s)))
}

fn as_user(w: &World, sub: &str) -> String {
    let claims = json!({"sub": sub, "email": format!("{sub}@example.com"), "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    sign_in(&w.addr)
}

fn csrf(w: &World, cookie: &str) -> String {
    let reply = get(&w.addr, "/account/api", Some(cookie));
    let view: Value = serde_json::from_str(&reply.body).unwrap();
    view["csrf"].as_str().unwrap().to_owned()
}

fn json_of(reply: &Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", reply.body))
}

/// Writes `body` to node `name` as the page of `cookie` would.
fn write(w: &World, cookie: &str, name: &str, body: &Value) -> Reply {
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(w, cookie)
    );
    send(
        &w.addr,
        "PUT",
        &format!("/account/api/nodes/{name}"),
        &extra,
        &body.to_string(),
    )
}

#[test]
fn the_page_sets_a_cap_and_labels_and_clears_the_cap_again() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let capped = json!({"max_boxes": 2, "labels": ["gpu"], "revision": 0});
    let saved = write(&w, &cookie, "hv", &capped);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let now = json_of(&saved);
    assert_eq!(
        (&now["max_boxes"], &now["revision"]),
        (&json!(2), &json!(1))
    );
    let listed = json_of(&get(&w.addr, "/account/api", Some(&cookie)));
    assert_eq!(listed["machines"]["nodes"][0]["max_boxes"], 2);
    assert_eq!(listed["machines"]["nodes"][0]["labels"], json!(["gpu"]));
    let clear = json!({"max_boxes": null, "labels": ["gpu"], "revision": 1});
    assert_eq!(write(&w, &cookie, "hv", &clear).status, 200);
    assert_eq!(w.node.lock().unwrap().max_boxes, None);
    let sent: Vec<Value> = w
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["cmd"] == "node-settings-set")
        .map(|r| r["request"].clone())
        .collect();
    assert_eq!(
        sent[0],
        json!({"cmd": "node-settings-set", "name": "hv", "max_boxes": {"set": 2},
            "labels": ["gpu"], "expected_revision": 0})
    );
    assert_eq!(sent[1]["max_boxes"], "clear");
}

#[test]
fn a_write_at_a_revision_that_has_moved_is_a_conflict_that_names_who_changed_it() {
    let w = world(true, &[]);
    let alice = as_user(&w, "alice-sub");
    let stale = as_user(&w, "alice-sub");
    let first = json!({"max_boxes": 1, "labels": [], "revision": 0});
    assert_eq!(write(&w, &alice, "hv", &first).status, 200);
    let again = json!({"max_boxes": 9, "labels": [], "revision": 0});
    let conflict = write(&w, &stale, "hv", &again);
    assert_eq!(conflict.status, 409, "{}", conflict.body);
    let body = json_of(&conflict);
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("alice-sub") && error.contains("2026-11-17T10:00:00Z"),
        "{error}"
    );
    assert_eq!(body["current"]["revision"], 1);
    assert_eq!(w.node.lock().unwrap().max_boxes, Some(1));
}

#[test]
fn the_controllers_reasons_come_back_with_a_status_that_says_whose_doing_it_is() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let bad_label = json!({"max_boxes": 1, "labels": ["bogus"], "revision": 0});
    let reply = write(&w, &cookie, "hv", &bad_label);
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(json_of(&reply)["error"].as_str().unwrap().contains("bogus"));
    let other = json!({"max_boxes": 1, "labels": [], "revision": 0});
    let reply = write(&w, &cookie, "elsewhere", &other);
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert_eq!(w.node.lock().unwrap().revision, 0);
}

#[test]
fn a_write_needs_the_csrf_header_a_cap_of_at_least_one_labels_and_a_revision() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let body = json!({"max_boxes": 1, "labels": [], "revision": 0}).to_string();
    for csrf in ["", "X-Moor-Csrf: forged\r\n"] {
        let extra = format!("Content-Type: application/json\r\nCookie: {cookie}\r\n{csrf}");
        let reply = send(&w.addr, "PUT", "/account/api/nodes/hv", &extra, &body);
        assert_eq!(reply.status, 403, "{}", reply.body);
    }
    let extra = "Content-Type: application/json\r\n".to_owned();
    let reply = send(&w.addr, "PUT", "/account/api/nodes/hv", &extra, &body);
    assert_eq!(reply.status, 401);
    for bad in [
        json!({"max_boxes": 0, "labels": [], "revision": 0}),
        json!({"max_boxes": 1, "revision": 0}),
        json!({"max_boxes": 1, "labels": []}),
    ] {
        let reply = write(&w, &cookie, "hv", &bad);
        assert_eq!(reply.status, 400, "{bad}: {}", reply.body);
    }
    assert_eq!(w.node.lock().unwrap().revision, 0);
}
