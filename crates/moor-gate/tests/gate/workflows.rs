//! The workflows routes under `/account/api/workflows`, against the testkit issuer and a fake
//! controller that holds one workflow of `alice-sub`, with a run that waits at an approve
//! step and a run that was delivered. Every string an agent or a person wrote is hostile.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::login::{get, send, sign_in, Reply};
use super::secrets::{world, World};

const OWNER: &str = "alice";
const HOSTILE: &str = "<script>alert(1)</script>";

/// What the fake controller holds of the owner's workflow.
struct Held {
    revision: u64,
    prompt: String,
    plan: Option<Value>,
    enabled: bool,
    on_pull_request: bool,
    waiting: bool,
    answered: Vec<String>,
}

/// One `Held` per signed-in account, so tests that run side by side do not meet.
static HELD: LazyLock<Mutex<HashMap<String, Held>>> = LazyLock::new(Mutex::default);

fn plan() -> Value {
    json!({"steps": [
        {"id": "audit", "title": format!("Audit {HOSTILE}"), "kind": "work",
         "instructions": "look", "done_when": "report exists", "evidence": ["report.md"],
         "ok": "test"},
        {"id": "test", "title": "Test", "kind": "check", "command": "make test", "ok": "major",
         "fail": "audit", "max_tries": 2},
        {"id": "major", "title": "Major bump", "kind": "approve",
         "question": format!("Bump {HOSTILE}?"), "ok": "ship"},
        {"id": "ship", "title": "Open the PR", "kind": "deliver", "instructions": "open it",
         "done_when": "a PR exists"}],
        "made_from": "0".repeat(64)})
}

fn fresh() -> Held {
    Held {
        revision: 3,
        prompt: format!("Update the deps {HOSTILE}"),
        plan: Some(plan()),
        enabled: true,
        on_pull_request: false,
        waiting: true,
        answered: Vec::new(),
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

#[allow(
    clippy::too_many_lines,
    reason = "one fixture run that has a line of every kind"
)]
fn waiting_run(held: &Held) -> Value {
    let (outcome, phase) = if held.waiting {
        ("waiting", "waiting")
    } else {
        ("finished", "done")
    };
    let mut history = vec![
        json!({"at": 1000, "entry": {"kind": "queued"}}),
        json!({"at": 2000, "entry": {"kind": "entered", "step": "audit", "try_n": 1}}),
        json!({"at": 3000, "entry": {"kind": "reported", "step": "audit", "try_n": 1,
            "report": {"step": "audit", "result": "ok", "evidence": ["report.md"],
                "note": format!("found {HOSTILE}")}}}),
        json!({"at": 4000, "entry": {"kind": "entered", "step": "test", "try_n": 1}}),
        json!({"at": 5000, "entry": {"kind": "checked", "step": "test", "try_n": 1, "exit": 0,
            "tail": format!("{}{HOSTILE}", "x".repeat(6000))}}),
        json!({"at": 6000, "entry": {"kind": "entered", "step": "major", "try_n": 1}}),
        json!({"at": 6500, "entry": {"kind": "tuned", "what": HOSTILE}}),
    ];
    if !held.waiting {
        history.push(
            json!({"at": 7000, "entry": {"kind": "answered", "step": "major",
            "answer": "yes"}}),
        );
        history.push(
            json!({"at": 8000, "entry": {"kind": "reported", "step": "ship",
            "try_n": 1, "report": {"step": "ship", "result": "ok", "evidence": [],
            "note": "PR 5"}}}),
        );
        history.push(
            json!({"at": 9000, "entry": {"kind": "ended", "phase_before": "running",
            "outcome": {"outcome": "delivered", "result": "PR 5"}}}),
        );
    }
    json!({"id": 7, "due_at": 1000, "outcome": outcome, "phase": phase,
        "step": if held.waiting { json!("major") } else { Value::Null },
        "result": if held.waiting { Value::Null } else { json!("PR 5") },
        "box": "weekly-deps-1", "history": history})
}

fn delivered_run() -> Value {
    json!({"id": 6, "due_at": 500, "outcome": "finished", "phase": "done",
        "result": "ok", "history": [
            {"at": 100, "entry": {"kind": "queued"}},
            {"at": 400_100, "entry": {"kind": "ended", "phase_before": "running",
                "outcome": {"outcome": "delivered", "result": "ok"}}}]})
}

/// What the fake controller answers a workflow request or an artifact read.
#[allow(
    clippy::too_many_lines,
    clippy::significant_drop_tightening,
    reason = "a fake controller that holds its state for the whole request"
)]
pub(super) fn answer(who: &str, cmd: &str, inner: &Value) -> Response {
    if !who.contains(OWNER) {
        return refusal(ErrorKind::Refused, "no workflow named weekly-deps");
    }
    let mut all = HELD.lock().unwrap();
    let held = all.entry(who.to_owned()).or_insert_with(fresh);
    let view = |held: &Held| {
        json!({"name": "weekly-deps", "cron": "0 6 * * 1", "timezone": "UTC",
            "repos": ["acme/web"], "prompt": held.prompt, "enabled": held.enabled,
            "timeout_minutes": 180, "next_due": 9_999_000, "plan": held.plan,
            "plan_stale": false, "revision": held.revision, "on_pull_request": held.on_pull_request,
            "last_run": waiting_run(held)})
    };
    match cmd {
        "workflow-list" => Response::ok("x", Some(json!({"workflows": [view(held)]}))),
        "workflow-run-list" => {
            let one = inner["run"].as_i64();
            let mut runs = vec![waiting_run(held), delivered_run()];
            runs.retain(|r| one.is_none_or(|id| r["id"] == id));
            if one.is_some() {
                let plan = held.plan.clone().unwrap_or(Value::Null);
                for run in &mut runs {
                    run["plan"] = plan.clone();
                }
            }
            Response::ok("x", Some(json!({"workflow": "weekly-deps", "runs": runs})))
        }
        "workflow-add" => {
            if inner["expected_revision"]
                .as_u64()
                .is_some_and(|r| r != held.revision)
            {
                return refusal(ErrorKind::Refused, "it changed after you read it");
            }
            inner["prompt"]
                .as_str()
                .unwrap()
                .clone_into(&mut held.prompt);
            held.on_pull_request = inner["on_pull_request"] == true;
            held.enabled = held.enabled || inner["keep_state"] != true;
            held.revision += 1;
            Response::ok("x", Some(json!({"name": "weekly-deps", "changed": true})))
        }
        "workflow-set-enabled" => {
            held.enabled = inner["enabled"].as_bool().unwrap();
            Response::ok("x", Some(json!({"name": "weekly-deps", "changed": true})))
        }
        "workflow-answer" => {
            held.answered
                .push(inner["answer"].as_str().unwrap().to_owned());
            held.waiting = false;
            Response::ok(
                "x",
                Some(json!({"workflow": "weekly-deps", "next": "went on"})),
            )
        }
        "workflow-run-now" => run_now::reply(held.waiting),
        _ => Response::ok("x", None),
    }
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

fn put(w: &World, cookie: &str, body: &Value) -> Reply {
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(w, cookie)
    );
    send(
        &w.addr,
        "PUT",
        "/account/api/workflows/weekly-deps",
        &extra,
        &body.to_string(),
    )
}

fn answer_run(w: &World, cookie: &str, what: &str, with_csrf: bool) -> Reply {
    let token = if with_csrf {
        csrf(w, cookie)
    } else {
        "forged".to_owned()
    };
    let extra =
        format!("Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {token}\r\n");
    let body = json!({"answer": what}).to_string();
    send(
        &w.addr,
        "POST",
        "/account/api/workflows/weekly-deps/runs/7/answer",
        &extra,
        &body,
    )
}

fn plan_text() -> String {
    serde_json::to_string_pretty(&plan()).unwrap()
}

#[test]
fn the_list_shows_each_workflow_with_its_last_run_s_status() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-1");
    let reply = get(&w.addr, "/account/api/workflows", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    let row = &json_of(&reply)["workflows"][0];
    assert_eq!(row["name"], "weekly-deps");
    assert_eq!(row["last"]["status"], "waiting");
    assert_eq!(row["last"]["label"], "waiting on you at major");
    assert_eq!(row["has_plan"], true);
    assert_eq!(row["next_due"], 9_999_000);
    assert_eq!(get(&w.addr, "/account/api/workflows", None).status, 401);
}

#[test]
fn the_page_lists_runs_with_icon_time_duration_and_outcome() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-2");
    let flow = json_of(&get(
        &w.addr,
        "/account/api/workflows/weekly-deps",
        Some(&cookie),
    ));
    assert_eq!(flow["plan_edited"], false);
    assert_eq!(flow["revision"], 3);
    let runs = flow["runs"].as_array().unwrap();
    assert_eq!(runs[0]["id"], 7);
    assert_eq!(runs[1]["status"], "success");
    assert_eq!(runs[1].get("repo"), None);
    assert_eq!(runs[1]["duration_ms"], 400_000);
    assert_eq!(runs[1]["label"], "finished");
    assert_eq!(runs[0]["duration_ms"], Value::Null);
}

#[test]
#[allow(
    clippy::cognitive_complexity,
    reason = "one run page read, asserted field by field"
)]
fn a_run_shows_every_step_kind_with_history_check_output_and_evidence() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-3");
    let reply = get(
        &w.addr,
        "/account/api/workflows/weekly-deps/runs/7",
        Some(&cookie),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let run = json_of(&reply);
    let steps = run["steps"].as_array().unwrap();
    let kinds: Vec<&str> = steps.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["work", "check", "approve", "deliver"]);
    let states: Vec<&str> = steps.iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["passed", "passed", "waiting", "pending"]);
    let work = &steps[0]["history"];
    assert_eq!(work[1]["evidence"][0]["name"], "report.md");
    assert_eq!(
        work[1]["evidence"][0]["href"],
        "/account/api/artifact?box=weekly-deps-1&name=report.md"
    );
    let tail = steps[1]["check"]["tail"].as_str().unwrap();
    assert_eq!(tail.len(), 4096);
    assert!(tail.ends_with(HOSTILE));
    assert_eq!(steps[1]["check"]["exit"], 0);
    assert_eq!(run["question"]["step"], "major");
    assert_eq!(run["question"]["text"], format!("Bump {HOSTILE}?"));
    assert_eq!(run["status"], "waiting");
    let loose: Vec<&str> = run["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect();
    assert_eq!(loose[0], "Queued; its Box is starting");
    assert!(loose.iter().any(|l| l.starts_with("tuned:")), "{loose:?}");
}

#[test]
fn text_from_people_and_agents_comes_back_as_inert_json_and_the_page_never_injects_html() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-4");
    for path in [
        "/account/api/workflows",
        "/account/api/workflows/weekly-deps",
        "/account/api/workflows/weekly-deps/runs/7",
    ] {
        let reply = get(&w.addr, path, Some(&cookie));
        assert_eq!(
            reply.header("content-type"),
            Some("application/json"),
            "{path}"
        );
        assert_eq!(
            reply.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
    }
    let flow = json_of(&get(
        &w.addr,
        "/account/api/workflows/weekly-deps",
        Some(&cookie),
    ));
    assert_eq!(flow["prompt"], format!("Update the deps {HOSTILE}"));
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/src");
    for entry in std::fs::read_dir(source).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for sink in [
            "dangerouslySetInnerHTML",
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
        ] {
            assert!(!text.contains(sink), "the web app uses {sink}");
        }
    }
}

#[test]
fn a_save_stores_the_prompt_alone_and_keeps_every_other_field() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-5");
    let body = json!({"prompt": "same prompt", "revision": 3});
    let reply = put(&w, &cookie, &body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let flow = json_of(&reply);
    assert_eq!(flow["revision"], 4);
    let seen = w.seen.lock().unwrap().clone();
    let add = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "workflow-add")
        .unwrap();
    assert_eq!(add["who"], "authgear:alice-5");
    assert_eq!(add["request"]["cron"], "0 6 * * 1");
    assert_eq!(add["request"]["repos"], json!(["acme/web"]));
    assert_eq!(add["request"]["timeout_minutes"], 180);
    assert_eq!(add["request"]["expected_revision"], 3);
    assert!(add["request"].get("plan").is_none(), "the page never writes a plan");
}

#[test]
fn a_save_that_leaves_the_plan_as_it_was_is_not_marked_edited() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-6");
    let body = json!({"prompt": "a new prompt", "plan": plan_text(), "revision": 3});
    let flow = json_of(&put(&w, &cookie, &body));
    assert_eq!(flow["prompt"], "a new prompt");
    assert_eq!(flow["plan_edited"], false);
}

#[test]
fn a_write_at_a_revision_that_has_moved_is_a_conflict_and_changes_nothing() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-7");
    let first = json!({"prompt": "first", "plan": plan_text(), "revision": 3});
    assert_eq!(put(&w, &cookie, &first).status, 200);
    let stale = json!({"prompt": "second", "plan": plan_text(), "revision": 3});
    let reply = put(&w, &cookie, &stale);
    assert_eq!(reply.status, 409, "{}", reply.body);
    let body = json_of(&reply);
    assert!(body["error"].as_str().unwrap().contains("Reload"));
    assert_eq!(body["current"]["revision"], 4);
    let flow = json_of(&get(
        &w.addr,
        "/account/api/workflows/weekly-deps",
        Some(&cookie),
    ));
    assert_eq!(flow["prompt"], "first");
    let fresh = json!({"prompt": "second", "plan": plan_text(), "revision": 4});
    assert_eq!(put(&w, &cookie, &fresh).status, 200);
}

#[test]
fn a_save_of_a_disabled_workflow_keeps_it_disabled() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-9");
    get(&w.addr, "/account/api/workflows/weekly-deps", Some(&cookie));
    HELD.lock()
        .unwrap()
        .iter_mut()
        .find(|(k, _)| k.contains("alice-9"))
        .unwrap()
        .1
        .enabled = false;
    let ok = json!({"prompt": "p", "plan": plan_text(), "revision": 3});
    let flow = json_of(&put(&w, &cookie, &ok));
    assert_eq!(flow["enabled"], false);
    let cmds = w.cmds();
    assert!(
        !cmds.contains(&"workflow-set-enabled".to_owned()),
        "a save is one controller request that keeps the state"
    );
    let seen = w.seen.lock().unwrap().clone();
    let add = seen
        .iter()
        .rev()
        .find(|r| r["request"]["cmd"] == "workflow-add")
        .unwrap();
    assert_eq!(add["request"]["keep_state"], true);
    assert!(add["request"].get("origin_key").is_none());
}

#[test]
fn a_write_needs_the_csrf_header_and_a_session() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-10");
    let body = json!({"prompt": "p", "plan": plan_text(), "revision": 3}).to_string();
    for csrf in ["", "X-Moor-Csrf: forged\r\n"] {
        let extra = format!("Content-Type: application/json\r\nCookie: {cookie}\r\n{csrf}");
        let reply = send(
            &w.addr,
            "PUT",
            "/account/api/workflows/weekly-deps",
            &extra,
            &body,
        );
        assert_eq!(reply.status, 403, "{}", reply.body);
    }
    let extra = "Content-Type: application/json\r\n".to_owned();
    let reply = send(
        &w.addr,
        "PUT",
        "/account/api/workflows/weekly-deps",
        &extra,
        &body,
    );
    assert_eq!(reply.status, 401);
    assert!(!w.cmds().contains(&"workflow-add".to_owned()));
}

#[test]
fn yes_answers_the_waiting_run_as_the_session_person_and_the_run_goes_on() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-11");
    let reply = answer_run(&w, &cookie, "yes", true);
    assert_eq!(reply.status, 200, "{}", reply.body);
    let run = json_of(&reply);
    assert_eq!(run["status"], "success");
    assert_eq!(run["question"], Value::Null);
    let seen = w.seen.lock().unwrap().clone();
    let sent = seen
        .iter()
        .find(|r| r["request"]["cmd"] == "workflow-answer")
        .unwrap();
    assert_eq!(sent["who"], "authgear:alice-11");
    assert_eq!(sent["request"]["workflow"], "weekly-deps");
    assert_eq!(sent["request"].get("repo"), None);
    assert_eq!(sent["request"]["answer"], "yes");
    assert!(sent["request"].get("identity").is_none_or(Value::is_null));
}

#[test]
fn only_the_owner_answers_and_a_forged_page_or_a_bad_answer_is_refused() {
    let w = world(true, &[]);
    let bob = as_user(&w, "bob-12");
    let read = get(
        &w.addr,
        "/account/api/workflows/weekly-deps/runs/7",
        Some(&bob),
    );
    assert_eq!(read.status, 404, "{}", read.body);
    let reply = answer_run(&w, &bob, "yes", true);
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert!(!w.cmds().contains(&"workflow-answer".to_owned()));
    let alice = as_user(&w, "alice-12");
    assert_eq!(answer_run(&w, &alice, "yes", false).status, 403);
    assert_eq!(answer_run(&w, &alice, "maybe", true).status, 400);
    assert!(!w.cmds().contains(&"workflow-answer".to_owned()));
    let none = send(
        &w.addr,
        "POST",
        "/account/api/workflows/weekly-deps/runs/7/answer",
        "",
        "{}",
    );
    assert_eq!(none.status, 401);
}

#[test]
fn an_answer_to_a_run_that_does_not_wait_is_a_conflict() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-13");
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(&w, &cookie)
    );
    let body = json!({"answer": "no"}).to_string();
    let reply = send(
        &w.addr,
        "POST",
        "/account/api/workflows/weekly-deps/runs/6/answer",
        &extra,
        &body,
    );
    assert_eq!(reply.status, 409, "{}", reply.body);
}

#[test]
fn an_unknown_run_is_not_found() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-14");
    let reply = get(
        &w.addr,
        "/account/api/workflows/weekly-deps/runs/99",
        Some(&cookie),
    );
    assert_eq!(reply.status, 404, "{}", reply.body);
}

#[test]
fn a_workflow_that_is_not_yours_or_does_not_exist_is_not_found_every_way() {
    let w = world(true, &[]);
    let bob = as_user(&w, "bob-15");
    let body = json!({"prompt": "p", "plan": plan_text(), "revision": 3});
    let put = put(&w, &bob, &body);
    assert_eq!(put.status, 404, "{}", put.body);
    let read = get(&w.addr, "/account/api/workflows/weekly-deps", Some(&bob));
    assert_eq!(read.status, 404, "{}", read.body);
    assert!(!w.cmds().contains(&"workflow-add".to_owned()));
    let alice = as_user(&w, "alice-15");
    let mine = get(&w.addr, "/account/api/workflows/weekly-deps", Some(&alice));
    assert_eq!(mine.status, 200, "{}", mine.body);
    let unknown = get(&w.addr, "/account/api/workflows/nope", Some(&alice));
    assert_eq!(unknown.status, 404, "{}", unknown.body);
}
mod pull;
mod run_now;
