//! The Boxes routes under `/account/api/box`, against the testkit issuer and a fake controller
//! that holds two Boxes of `alice-sub`. The state is the controller's: the gate carries it.

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::login::{get, send, sign_in};
use super::secrets::world;

/// Accounts whose subject starts with this reach this fake; the others reach the shared one.
pub(super) const PREFIX: &str = "portal-";
const OWNER: &str = "portal-alice";
const HOSTILE: &str = "<script>alert(1)</script>";

fn row(name: &str, lifecycle: &str, state: &str, line: &str, pr: Value) -> Value {
    json!({"id": 1, "name": name, "repo": "acme/web", "issue": null, "prompt": "Fix the typo",
        "lifecycle": lifecycle, "session_state": "working", "state_reason": null,
        "base": "main", "branch": "moor/1-fix", "driver": null, "agent": null, "run": null,
        "updated_at": "2026-11-18T10:00:00Z",
        "progress": {"title": "Fix the typo", "state": state, "stale": false, "line": line,
                 "last_event_at": "2026-11-18T10:00:00Z", "pr": pr}})
}

fn refusal(message: &str) -> Response {
    let view = ErrorView {
        kind: ErrorKind::Failed,
        message: message.to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    Response::err("x", view)
}

/// What the fake controller answers `show` and `artifacts`.
pub(super) fn answer(who: &str, cmd: &str, inner: &Value) -> Response {
    let mine = who.contains(OWNER);
    let wanted = inner["box"].as_str();
    match (cmd, wanted) {
        ("show", None) => {
            let boxes = if mine {
                vec![
                    row(
                        "web/ask",
                        "ready",
                        "needs_you",
                        "Allow npm run e2e?",
                        Value::Null,
                    ),
                    row(
                        "web/done",
                        "ready",
                        "done",
                        "PR #212",
                        json!({"repo": "acme/web", "number": 212}),
                    ),
                ]
            } else {
                vec![]
            };
            Response::ok("x", Some(json!({"boxes": boxes, "box": null})))
        }
        ("show", Some(name)) if mine && name == "web/ask" => {
            let mut boxed = row(name, "ready", "needs_you", HOSTILE, Value::Null);
            boxed["prompt"] = json!(HOSTILE);
            Response::ok(
                "x",
                Some(json!({"box": boxed, "events": [], "timeline": [
                    {"id": 1, "at": "2026-11-18T10:00:00Z", "kind": "agent_said",
                     "class": "message", "text": HOSTILE}],
                    "artifacts": [
                        {"name": "after.png", "version": 2, "kind": "screenshot",
                         "content_type": "image/png", "size": 10, "sha256": "0",
                         "created_at": "2026-11-18T10:00:00Z"},
                        {"name": "x y&z.svg", "version": 1, "kind": "screenshot",
                         "content_type": "image/svg+xml", "size": 10, "sha256": "0",
                         "created_at": "2026-11-18T10:00:00Z"}]})),
            )
        }
        ("show", Some(name)) => refusal(&format!("no Box {name}")),
        ("artifacts", _) => Response::ok("x", Some(json!([]))),
        _ => refusal("unexpected"),
    }
}

fn user(w: &super::secrets::World, sub: &str) -> String {
    let claims = json!({"sub": sub, "email": format!("{sub}@example.com"), "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    sign_in(&w.addr)
}

fn json_of(reply: &super::login::Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap()
}

#[test]
fn the_list_carries_the_state_the_controller_derived_and_a_pr_link_built_from_its_parts() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-1");
    let reply = get(&w.addr, "/account/api/boxes", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let rows = json_of(&reply)["boxes"].clone();
    let states: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["needs_you", "done"]);
    assert_eq!(rows[0]["title"], "Fix the typo");
    assert_eq!(rows[0]["line"], "Allow npm run e2e?");
    assert_eq!(rows[0]["pr"], Value::Null);
    assert_eq!(rows[1]["pr"]["url"], "https://github.com/acme/web/pull/212");
}

#[test]
fn one_box_has_its_timeline_and_artifacts_and_hostile_text_stays_inert_json() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-2");
    let reply = get(&w.addr, "/account/api/box?box=web%2Fask", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let detail = json_of(&reply);
    assert_eq!(detail["line"], HOSTILE);
    assert_eq!(detail["timeline"][0]["text"], HOSTILE);
    let art = detail["artifacts"].as_array().unwrap();
    assert_eq!(art[0]["image"], true);
    assert_eq!(
        art[0]["href"],
        "/account/api/box/artifact?box=web%2Fask&name=after.png&version=2"
    );
    assert_eq!(
        art[1]["image"], false,
        "an SVG can carry script, so it is never inline"
    );
    assert!(art[1]["href"].as_str().unwrap().contains("x+y%26z.svg"));
}

#[test]
fn someone_elses_box_is_not_found_and_their_list_is_empty() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-bob-3");
    let reply = get(&w.addr, "/account/api/box?box=web%2Fask", Some(&cookie));
    assert_eq!(reply.status, 404, "{}", reply.body);
    let list = get(&w.addr, "/account/api/boxes", Some(&cookie));
    assert_eq!(json_of(&list)["boxes"], json!([]));
    let artifact = get(
        &w.addr,
        "/account/api/box/artifact?box=web%2Fask&name=after.png",
        Some(&cookie),
    );
    assert_eq!(artifact.status, 404, "{}", artifact.body);
}

#[test]
fn signed_out_gets_401_and_a_bad_request_gets_400() {
    let w = world(true, &[]);
    for path in [
        "/account/api/boxes",
        "/account/api/box?box=web%2Fask",
        "/account/api/box/artifact?box=a&name=b",
    ] {
        assert_eq!(get(&w.addr, path, None).status, 401, "{path}");
    }
    let cookie = user(&w, "portal-alice-4");
    assert_eq!(get(&w.addr, "/account/api/box", Some(&cookie)).status, 400);
    let bad = get(
        &w.addr,
        "/account/api/box/artifact?box=web%2Fask&name=..%2Fx",
        Some(&cookie),
    );
    assert_eq!(bad.status, 400, "{}", bad.body);
}

#[test]
fn the_app_page_may_load_images_and_fonts_from_itself_and_nothing_else() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-5");
    let page = get(&w.addr, "/", Some(&cookie));
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("img-src 'self';"), "{csp}");
    assert!(csp.contains("font-src 'self';"), "{csp}");
    assert!(csp.contains("default-src 'none'"), "{csp}");
}

fn post(
    w: &super::secrets::World,
    cookie: &str,
    csrf: Option<&str>,
    body: &str,
) -> super::login::Reply {
    let token = csrf.map_or_else(String::new, |c| format!("X-Moor-Csrf: {c}\r\n"));
    let extra = format!("Cookie: {cookie}\r\n{token}Content-Type: application/json\r\n");
    send(&w.addr, "POST", "/account/api/boxes", &extra, body)
}

fn csrf_of(w: &super::secrets::World, cookie: &str) -> String {
    let view = json_of(&get(&w.addr, "/account/api", Some(cookie)));
    view["csrf"].as_str().unwrap().to_owned()
}

#[test]
fn starting_a_box_from_the_portal_needs_the_page_s_csrf_token_and_a_prompt_and_a_repo() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-6");
    let ok = json!({"repo": "acme/web", "prompt": "Fix the typo"}).to_string();
    assert_eq!(post(&w, &cookie, None, &ok).status, 403);
    assert_eq!(post(&w, &cookie, Some("wrong"), &ok).status, 403);
    let csrf = csrf_of(&w, &cookie);
    for bad in [
        "not json",
        r#"{"repo":"acme/web","prompt":"  "}"#,
        r#"{"repo":"","prompt":"x"}"#,
    ] {
        let reply = post(&w, &cookie, Some(&csrf), bad);
        assert_eq!(reply.status, 400, "{bad}: {}", reply.body);
    }
    assert!(
        w.cmds().iter().all(|c| c != "start"),
        "nothing reached the controller"
    );
    let out = send(
        &w.addr,
        "POST",
        "/account/api/boxes",
        "Content-Type: application/json\r\n",
        &ok,
    );
    assert_eq!(out.status, 401);
}

#[test]
fn a_portal_start_sends_the_start_request_start_box_sends_and_a_refusal_is_readable() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-7");
    let csrf = csrf_of(&w, &cookie);
    let body =
        json!({"repo": "acme/web", "prompt": "Fix the typo", "base": "main", "node": "engine-03"});
    let reply = post(&w, &cookie, Some(&csrf), &body.to_string());
    assert_eq!(reply.status, 400, "{}", reply.body);
    let why = json_of(&reply)["error"].as_str().unwrap().to_owned();
    assert!(why.contains("missing secrets claude, github"), "{why}");
    let start = w
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r["request"]["cmd"] == "start")
        .cloned()
        .expect("a start");
    assert_eq!(start["request"]["repo"], "acme/web");
    assert_eq!(start["request"]["prompt"], "Fix the typo");
    assert_eq!(start["request"]["base"], "main");
    assert_eq!(start["request"]["node"], "engine-03");
}
