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

/// The gates of the Box that asks: one gate, open and waiting for its owner.
fn gates() -> Value {
    json!({"gates": ["review"], "wait": null,
        "status": {"state": "open", "gate": "review", "try_n": 1, "next": "ask"}})
}

fn row(name: &str, lifecycle: &str, state: &str, line: &str, pr: Value) -> Value {
    let gate = if name == "web/ask" {
        gates()
    } else {
        Value::Null
    };
    let (issue, ties) = match name {
        "web/done" => (
            json!("ACME-12"),
            json!({"workflow": {"name": "review", "pull": "acme/web#65"}, "machine": "big"}),
        ),
        _ => (Value::Null, json!({})),
    };
    json!({"gate": gate, "id": 1, "name": name, "repo": "acme/web", "issue": issue, "ties": ties, "prompt": "Fix the typo",
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

/// The Box whose setup failed, as `show` sends it.
fn failed_box(name: &str) -> Response {
    let line = "failed: clone timed out";
    let mut boxed = row(name, "setup_failed", "failed", line, Value::Null);
    boxed["session_state"] = json!("starting");
    boxed["ties"] = json!({"workflow": {"name": "nightly"}});
    let shown = json!({"box": boxed, "events": [], "timeline": [], "artifacts": []});
    Response::ok("x", Some(shown))
}

/// A workflow whose latest run was refused for the failed Box.
fn workflows() -> Response {
    let run = json!({"due_at": 1, "outcome": "failed", "phase": "done",
        "planned_box": "nightly/w1-202611190600"});
    let flow = json!({"name": "nightly", "cron": "0 6 * * *", "timezone": "UTC",
        "repos": ["acme/web"], "prompt": "p", "enabled": true, "timeout_minutes": 60,
        "last_run": run});
    Response::ok("x", Some(json!({"workflows": [flow]})))
}

/// A question's task whose place started a work Box, as `show` sends it.
fn place(name: &str) -> Response {
    let mut boxed = row(name, "ready", "done", "answered", Value::Null);
    boxed["task_at"] = json!("2026-11-24T10:58:00Z");
    let work = json!({"id": 5, "name": "web/mcp-1a2b3c4d", "repo": "acme/web",
        "started_at": "2026-11-24T11:05:10Z", "ready_ms": 38000, "state": "closed",
        "closed_at": "2026-11-24T11:11:12Z",
        "artifacts": [{"name": "fee line.png", "version": 1, "kind": "screenshot",
            "content_type": "image/png", "size": 10, "sha256": "0",
            "created_at": "2026-11-24T11:10:00Z"}]});
    let shown = json!({"box": boxed, "events": [], "timeline": [], "artifacts": [],
        "task_boxes": [work], "outputs": [{"kind": "github.comment",
            "url": "https://example.com/acme/web/pull/3#c1", "title": "Evidence",
            "first_at": 1, "last_at": 2}]});
    Response::ok("x", Some(shown))
}

/// A task whose place started two work Boxes of one name, the first closed before the second.
fn twice(name: &str) -> Response {
    let boxed = row(name, "ready", "done", "answered", Value::Null);
    let work = |id: i64, file: &str| {
        json!({"id": id, "name": "web/w", "repo": "acme/web",
            "started_at": "2026-11-24T11:05:10Z", "state": "closed",
            "artifacts": [{"name": file, "version": 1, "kind": "file",
                "content_type": "text/plain", "size": 3, "sha256": "0",
                "created_at": "2026-11-24T11:10:00Z"}]})
    };
    let shown = json!({"box": boxed, "events": [], "timeline": [], "artifacts": [],
        "task_boxes": [work(7, "a.txt"), work(8, "b.txt")]});
    Response::ok("x", Some(shown))
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
        ("show", Some(name)) if mine && name == "web/place" => place(name),
        ("show", Some(name)) if mine && name == "web/twice" => twice(name),
        ("show", Some(name)) if mine && name == "nightly/w1-202611190600" => failed_box(name),
        ("show", Some(name)) => refusal(&format!("no Box {name}")),
        ("retry", Some("nightly/w1-202611190600")) if mine => {
            Response::ok("x", Some(json!({"retried": true})))
        }
        ("retry", _) => refusal("only a Box whose setup failed can be retried"),
        ("steer" | "sleep" | "close", wanted) => super::steer::answer(mine, cmd, wanted, inner),
        ("workflow-list", _) => workflows(),
        ("artifacts", _) => Response::ok("x", Some(json!([]))),
        _ => refusal("unexpected"),
    }
}

pub(super) fn user(w: &super::secrets::World, sub: &str) -> String {
    let claims = json!({"sub": sub, "email": format!("{sub}@example.com"), "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    sign_in(&w.addr)
}

pub(super) fn json_of(reply: &super::login::Reply) -> Value {
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
fn a_question_s_page_carries_its_work_boxes_with_hrefs_naming_the_work_box() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-9");
    let reply = get(&w.addr, "/account/api/box?box=web%2Fplace", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let detail = json_of(&reply);
    let work = &detail["task_boxes"][0];
    assert_eq!(work["name"], "web/mcp-1a2b3c4d");
    assert_eq!(work["repo"], "acme/web");
    assert_eq!(work["ready_ms"], 38000);
    assert_eq!(work["state"], "closed");
    assert_eq!(work["closed_at"], "2026-11-24T11:11:12Z");
    let art = &work["artifacts"][0];
    assert_eq!(art["image"], true);
    assert_eq!(
        art["href"], "/account/api/box/artifact?box_id=5&name=fee+line.png&version=1",
        "the picture is served by the work Box, not by the place"
    );
    assert_eq!(detail["artifacts"], json!([]));
}

#[test]
fn two_work_boxes_of_one_name_each_link_and_open_their_own_file() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-11");
    let reply = get(&w.addr, "/account/api/box?box=web%2Ftwice", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let detail = json_of(&reply);
    for (n, (id, file)) in [(7, "a.txt"), (8, "b.txt")].into_iter().enumerate() {
        let line = &detail["task_boxes"][n];
        let files: Vec<&str> = line["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(files, [file]);
        let href = line["artifacts"][0]["href"].as_str().unwrap();
        assert_eq!(
            href,
            format!("/account/api/box/artifact?box_id={id}&name={file}&version=1")
        );
        get(&w.addr, href, Some(&cookie));
        let asked = w
            .seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r["request"]["cmd"] == "artifacts")
            .cloned()
            .unwrap();
        assert_eq!(asked["request"]["box"], json!({"id": id}), "{file}");
    }
}

#[test]
fn a_task_s_page_carries_its_outputs_and_a_box_without_any_an_empty_list() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-12");
    let place = json_of(&get(
        &w.addr,
        "/account/api/box?box=web%2Fplace",
        Some(&cookie),
    ));
    assert_eq!(
        place["outputs"],
        json!([{"kind": "github.comment", "url": "https://example.com/acme/web/pull/3#c1",
            "title": "Evidence", "first_at": 1, "last_at": 2}])
    );
    let plain = json_of(&get(
        &w.addr,
        "/account/api/box?box=web%2Fask",
        Some(&cookie),
    ));
    assert_eq!(plain["outputs"], json!([]));
}

#[test]
fn a_box_with_no_work_boxes_sends_an_empty_list() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-10");
    let reply = get(&w.addr, "/account/api/box?box=web%2Fask", Some(&cookie));
    assert_eq!(json_of(&reply)["task_boxes"], json!([]));
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
    post_to(w, "/account/api/boxes", cookie, csrf, body)
}

pub(super) fn post_to(
    w: &super::secrets::World,
    path: &str,
    cookie: &str,
    csrf: Option<&str>,
    body: &str,
) -> super::login::Reply {
    let token = csrf.map_or_else(String::new, |c| format!("X-Moor-Csrf: {c}\r\n"));
    let extra = format!("Cookie: {cookie}\r\n{token}Content-Type: application/json\r\n");
    send(&w.addr, "POST", path, &extra, body)
}

pub(super) fn csrf_of(w: &super::secrets::World, cookie: &str) -> String {
    let view = json_of(&get(&w.addr, "/account/api", Some(cookie)));
    view["csrf"].as_str().unwrap().to_owned()
}

#[test]
fn starting_a_box_from_the_portal_needs_the_page_s_csrf_token_and_a_prompt_and_a_repo() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-6");
    let ok = json!({"repos": ["acme/web"], "prompt": "Fix the typo"}).to_string();
    assert_eq!(post(&w, &cookie, None, &ok).status, 403);
    assert_eq!(post(&w, &cookie, Some("wrong"), &ok).status, 403);
    let csrf = csrf_of(&w, &cookie);
    for bad in [
        "not json",
        r#"{"repos":["acme/web"],"prompt":"  "}"#,
        r#"{"repos":[],"prompt":"x"}"#,
        r#"{"repo":"acme/web","prompt":"x"}"#,
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
    let body = json!({"repos": ["acme/web", "acme/lib"], "prompt": "Fix the typo", "base": "main", "node": "engine-03"});
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
    assert_eq!(start["request"]["also"], json!(["acme/lib"]));
    assert_eq!(start["request"]["prompt"], "Fix the typo");
    assert_eq!(start["request"]["base"], "main");
    assert_eq!(start["request"]["node"], "engine-03");
}

#[test]
fn a_box_with_gates_carries_them_to_the_list_and_its_page_as_show_sent_them() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-11");
    let rows = json_of(&get(&w.addr, "/account/api/boxes", Some(&cookie)))["boxes"].clone();
    assert_eq!(rows[0]["gate"], gates());
    assert_eq!(rows[1]["gate"], Value::Null, "a Box without gates has none");
    let page = json_of(&get(
        &w.addr,
        "/account/api/box?box=web%2Fask",
        Some(&cookie),
    ));
    assert_eq!(page["gate"]["status"]["next"], "ask");
}

#[test]
fn a_row_carries_its_issue_machine_and_the_workflow_run_for_a_pull_request_that_made_it() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-12");
    let rows = json_of(&get(&w.addr, "/account/api/boxes", Some(&cookie)))["boxes"].clone();
    let done = &rows[1];
    assert_eq!(done["issue"], "ACME-12");
    assert_eq!(done["machine"], "big");
    assert_eq!(
        done["workflow"],
        json!({"name": "review", "pull": "acme/web#65"})
    );
    for key in ["issue", "machine", "workflow"] {
        assert_eq!(rows[0][key], Value::Null, "a plain Box has no {key}");
    }
}

#[test]
fn a_box_whose_setup_failed_says_so_and_names_the_workflow_run_that_made_it() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-8");
    let failed = json_of(&get(
        &w.addr,
        "/account/api/box?box=nightly%2Fw1-202611190600",
        Some(&cookie),
    ));
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["retryable"], true);
    assert_eq!(failed["run_again"], json!({"name": "nightly"}));
    assert_eq!(failed["workflow"], json!({"name": "nightly"}));
    let asking = json_of(&get(
        &w.addr,
        "/account/api/box?box=web%2Fask",
        Some(&cookie),
    ));
    assert_eq!(asking["retryable"], false);
    assert_eq!(
        asking["run_again"],
        Value::Null,
        "a Box still working is not run again"
    );
}

#[test]
fn a_portal_retry_needs_the_csrf_token_and_a_box_and_sends_the_retry_request() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-9");
    let retry =
        |csrf: Option<&str>, body: &str| post_to(&w, "/account/api/box/retry", &cookie, csrf, body);
    let ok = json!({"box": "nightly/w1-202611190600"}).to_string();
    assert_eq!(retry(None, &ok).status, 403);
    assert_eq!(retry(Some("wrong"), &ok).status, 403);
    let csrf = csrf_of(&w, &cookie);
    for bad in ["not json", r#"{"box": ""}"#, r#"{"box": "../x"}"#] {
        assert_eq!(retry(Some(&csrf), bad).status, 400, "{bad}");
    }
    assert!(
        w.cmds().iter().all(|c| c != "retry"),
        "nothing reached the controller"
    );
}

#[test]
fn a_portal_retry_sends_the_retry_request_and_a_refusal_is_readable() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-10");
    let csrf = csrf_of(&w, &cookie);
    let retry = |body: &str| post_to(&w, "/account/api/box/retry", &cookie, Some(&csrf), body);
    let ok = json!({"box": "nightly/w1-202611190600"}).to_string();
    let reply = retry(&ok);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply)["box"], "nightly/w1-202611190600");
    let sent = w
        .seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r["request"]["cmd"] == "retry")
        .cloned();
    assert_eq!(
        sent.expect("a retry")["request"]["box"],
        "nightly/w1-202611190600"
    );
    let refused = retry(r#"{"box": "web/done"}"#);
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(json_of(&refused)["error"]
        .as_str()
        .unwrap()
        .contains("setup failed"));
}
