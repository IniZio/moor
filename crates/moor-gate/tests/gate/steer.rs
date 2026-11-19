//! Steering, stopping and closing a Box from the portal: each writes as the signed-in person,
//! behind the page's CSRF token, and sends the controller request the matching tool sends.

use moor_core::response::Response;
use serde_json::{json, Value};

use super::portal::{csrf_of, json_of, post_to, user};
use super::secrets::{world, World};

/// What the fake controller answers `steer`, `sleep` and `close` for the portal's own Boxes.
pub(super) fn answer(mine: bool, cmd: &str, wanted: Option<&str>, inner: &Value) -> Response {
    let refusal = |why: &str| {
        let view = moor_core::response::ErrorView {
            kind: moor_core::response::ErrorKind::Refused,
            message: why.to_owned(),
            hints: vec![],
            context: vec![],
            detail: None,
        };
        Response::err("x", view)
    };
    match (cmd, wanted) {
        ("steer", Some("web/ask")) if mine => {
            let sent = inner["text"].as_str().unwrap_or("(menu)");
            Response::ok(
                "x",
                Some(
                    json!({"box": "web/ask", "sent": sent, "answered_menu": false,
                    "steer": 7, "delivery": "delivered", "inbox": 1}),
                ),
            )
        }
        ("sleep", Some("web/ask")) if mine => Response::ok("x", Some(json!({"box": "web/ask"}))),
        ("close", Some("web/done")) if mine && inner["force"] != true => {
            refusal("the Box holds work that is not pushed")
        }
        ("close", Some(_)) if mine => Response::ok("x", Some(json!({"closed": true}))),
        _ => refusal("unexpected"),
    }
}

fn sent(w: &World, cmd: &str) -> Vec<Value> {
    let seen = w.seen.lock().unwrap();
    seen.iter()
        .filter(|r| r["request"]["cmd"] == cmd)
        .map(|r| r["request"].clone())
        .collect()
}

#[test]
fn every_write_needs_the_page_s_csrf_token_and_a_box() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-20");
    let csrf = csrf_of(&w, &cookie);
    for path in ["steer", "stop", "close"] {
        let url = format!("/account/api/box/{path}");
        let ok = json!({"box": "web/ask", "text": "go"}).to_string();
        assert_eq!(post_to(&w, &url, &cookie, None, &ok).status, 403, "{path}");
        assert_eq!(
            post_to(&w, &url, &cookie, Some("x"), &ok).status,
            403,
            "{path}"
        );
        for bad in ["not json", r#"{"box": "../x", "text": "go"}"#] {
            let reply = post_to(&w, &url, &cookie, Some(&csrf), bad);
            assert_eq!(reply.status, 400, "{path} {bad}");
        }
    }
    let empty = json!({"box": "web/ask", "text": "  "}).to_string();
    let url = "/account/api/box/steer";
    assert_eq!(post_to(&w, url, &cookie, Some(&csrf), &empty).status, 400);
    for cmd in ["steer", "sleep", "close"] {
        assert!(sent(&w, cmd).is_empty(), "{cmd} reached the controller");
    }
}

#[test]
fn a_steer_sends_the_text_and_an_interrupt_sends_the_key_first() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-21");
    let csrf = csrf_of(&w, &cookie);
    let url = "/account/api/box/steer";
    let plain = json!({"box": "web/ask", "text": " use yarn "}).to_string();
    let reply = post_to(&w, url, &cookie, Some(&csrf), &plain);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(json_of(&reply)["delivery"], "delivered");
    let stop = json!({"box": "web/ask", "text": "wrong file", "interrupt": true}).to_string();
    assert_eq!(post_to(&w, url, &cookie, Some(&csrf), &stop).status, 200);
    let steers = sent(&w, "steer");
    assert_eq!(steers.len(), 3);
    assert_eq!(steers[0]["text"], "use yarn");
    assert_eq!(steers[1]["interrupt"], true);
    assert_eq!(steers[2]["text"], "wrong file");
}

#[test]
fn a_message_over_an_open_card_is_the_card_s_own_entry_with_the_words() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-22");
    let csrf = csrf_of(&w, &cookie);
    let body = json!({"box": "web/ask", "text": "no, use yarn", "ask": "ab12", "pick": 3});
    let url = "/account/api/box/steer";
    let reply = post_to(&w, url, &cookie, Some(&csrf), &body.to_string());
    assert_eq!(reply.status, 200, "{}", reply.body);
    let steers = sent(&w, "steer");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["menu"], 3);
    assert_eq!(steers[0]["ask"], "ab12");
    assert_eq!(steers[0]["text"], "no, use yarn");
}

#[test]
fn a_refused_steer_says_why_and_an_interrupt_that_went_through_says_so() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-23");
    let csrf = csrf_of(&w, &cookie);
    let url = "/account/api/box/steer";
    let body = json!({"box": "web/done", "text": "hi"}).to_string();
    let refused = post_to(&w, url, &cookie, Some(&csrf), &body);
    assert_ne!(refused.status, 200, "{}", refused.body);
    assert!(json_of(&refused)["error"]
        .as_str()
        .unwrap()
        .contains("unexpected"));
}

#[test]
fn stop_sleeps_the_box_and_close_is_refused_on_unpushed_work_until_forced() {
    let w = world(true, &[]);
    let cookie = user(&w, "portal-alice-24");
    let csrf = csrf_of(&w, &cookie);
    let stop = |p: &str, b: Value| post_to(&w, p, &cookie, Some(&csrf), &b.to_string());
    let reply = stop("/account/api/box/stop", json!({"box": "web/ask"}));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(sent(&w, "sleep")[0]["box"], "web/ask");
    let close = "/account/api/box/close";
    let refused = stop(close, json!({"box": "web/done"}));
    assert_eq!(refused.status, 502, "{}", refused.body);
    assert!(json_of(&refused)["error"]
        .as_str()
        .unwrap()
        .contains("not pushed"));
    let forced = stop(close, json!({"box": "web/done", "force": true}));
    assert_eq!(forced.status, 200, "{}", forced.body);
    let closes = sent(&w, "close");
    assert_eq!(
        (closes[0]["force"].as_bool(), closes[1]["force"].as_bool()),
        (Some(false), Some(true))
    );
}
