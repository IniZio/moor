#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test assertions index JSON values"
)]

use moor_core::request::{SecretValue, UserCmd};
use moor_core::user::UserId;
use moor_core::ErrorKind;
use serde_json::{json, Value};

use crate::error::CommandError;
use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};
use crate::store::{webhook_header_name, webhook_key_name};

fn add(url: &str, events: Option<Vec<&str>>) -> UserCmd {
    UserCmd::WebhookAdd {
        url: url.to_owned(),
        events: events.map(|e| e.into_iter().map(str::to_owned).collect()),
        header: None,
    }
}

fn add_with_header(header: &str) -> UserCmd {
    UserCmd::WebhookAdd {
        url: "https://example.com/h".to_owned(),
        events: None,
        header: Some(header.to_owned()),
    }
}

fn show(id: &Value) -> UserCmd {
    UserCmd::WebhookSetupShow {
        id: id.as_i64().unwrap(),
    }
}

fn finish(id: &Value, value: Option<&str>) -> UserCmd {
    UserCmd::WebhookSetupFinish {
        id: id.as_i64().unwrap(),
        header_value: value.map(|v| SecretValue::new(v.as_bytes().to_vec())),
    }
}

fn enrolled(f: &Fixture, who: &str) -> (moor_core::user::Identity, UserId) {
    let me = id(who);
    let (user, _) = f.cx.store().unwrap().enroll(&me, None).unwrap();
    (me, user)
}

fn refusal(r: Result<Value, CommandError>) -> String {
    let err = r.unwrap_err();
    assert!(
        matches!(err.kind, ErrorKind::Refused | ErrorKind::Usage),
        "{err:?}"
    );
    err.error.to_string()
}

#[test]
fn add_replies_with_an_id_waiting_for_setup_and_never_a_key() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let a = run(&f.cx, &f.vault, &me, &add("https://example.com/hook", None)).unwrap();
    let b = run(&f.cx, &f.vault, &me, &add("https://example.com/hook", None)).unwrap();
    assert_eq!(a, json!({"id": a["id"], "state": "waiting_for_secret"}));
    assert_ne!(a["id"], b["id"]);
    let ka = run(&f.cx, &f.vault, &me, &show(&a["id"])).unwrap()["key"].clone();
    let kb = run(&f.cx, &f.vault, &me, &show(&b["id"])).unwrap()["key"].clone();
    let key = ka.as_str().unwrap();
    assert_eq!(key.len(), 64);
    assert!(key.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')));
    assert_ne!(ka, kb);
    let listed = run(&f.cx, &f.vault, &me, &UserCmd::WebhookList).unwrap();
    assert_eq!(
        listed["webhooks"][0]["events"],
        json!([
            "idled",
            "needs_input",
            "closed",
            "setup_failed",
            "session_failed",
            "launch_timed_out",
            "relaunch_capped",
            "box_failed"
        ])
    );
}

#[test]
fn list_shows_only_the_callers_webhooks_and_never_a_key() {
    let f = fixture();
    let (alice, _) = enrolled(&f, "local:2000");
    let (bob, _) = enrolled(&f, "local:2001");
    let mine = run(
        &f.cx,
        &f.vault,
        &alice,
        &add("https://a.example/h", Some(vec!["closed"])),
    )
    .unwrap();
    run(&f.cx, &f.vault, &bob, &add("https://b.example/h", None)).unwrap();
    let key = run(&f.cx, &f.vault, &alice, &show(&mine["id"])).unwrap()["key"].clone();
    let listed = run(&f.cx, &f.vault, &alice, &UserCmd::WebhookList).unwrap();
    let all = listed["webhooks"].as_array().unwrap();
    assert_eq!(all.len(), 1);
    let one = &all[0];
    assert_eq!(one["id"], mine["id"]);
    assert_eq!(one["url"], "https://a.example/h");
    assert_eq!(one["events"], json!(["closed"]));
    assert!(one["created_at"].is_i64());
    for field in ["last_ok_at", "failing_since", "last_error", "header"] {
        assert!(one[field].is_null(), "{one}");
    }
    assert!(one.get("key").is_none(), "{one}");
    assert!(!listed.to_string().contains(key.as_str().unwrap()));
}

#[test]
fn remove_deletes_own_and_refuses_another_users_or_a_missing_id_alike() {
    let f = fixture();
    let (alice, _) = enrolled(&f, "local:2000");
    let (bob, _) = enrolled(&f, "local:2001");
    let theirs = run(&f.cx, &f.vault, &bob, &add("https://b.example/h", None)).unwrap();
    let mine = run(&f.cx, &f.vault, &alice, &add("https://a.example/h", None)).unwrap();
    let remove = |id: &Value| UserCmd::WebhookRemove {
        id: id.as_i64().unwrap(),
    };
    let other = refusal(run(&f.cx, &f.vault, &alice, &remove(&theirs["id"])));
    let missing = refusal(run(
        &f.cx,
        &f.vault,
        &alice,
        &UserCmd::WebhookRemove { id: 9999 },
    ));
    assert_eq!(other, missing);
    assert!(other.contains("no such webhook"), "{other}");
    let gone = run(&f.cx, &f.vault, &alice, &remove(&mine["id"])).unwrap();
    assert_eq!(gone, json!({"id": mine["id"]}));
    let listed = run(&f.cx, &f.vault, &alice, &UserCmd::WebhookList).unwrap();
    assert_eq!(listed["webhooks"], json!([]));
    let bobs = run(&f.cx, &f.vault, &bob, &UserCmd::WebhookList).unwrap();
    assert_eq!(bobs["webhooks"].as_array().unwrap().len(), 1);
}

#[test]
fn urls_follow_the_https_or_loopback_rule() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    for ok in [
        "https://example.com/h",
        "http://127.0.0.1:9000/h",
        "http://localhost/h",
        "http://[::1]:8080/h",
    ] {
        run(&f.cx, &f.vault, &me, &add(ok, None)).unwrap_or_else(|e| panic!("{ok}: {e:?}"));
    }
    for bad in [
        "http://example.com/h",
        "ftp://example.com",
        "https://",
        "example.com",
        "",
    ] {
        let msg = refusal(run(&f.cx, &f.vault, &me, &add(bad, None)));
        assert!(msg.contains("url"), "{bad}: {msg}");
    }
}

#[test]
fn events_must_be_real_kinds_and_within_bounds() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let url = "https://example.com/h";
    let unknown = refusal(run(
        &f.cx,
        &f.vault,
        &me,
        &add(url, Some(vec!["idled", "nope"])),
    ));
    assert!(unknown.contains("nope"), "{unknown}");
    let empty = refusal(run(&f.cx, &f.vault, &me, &add(url, Some(vec![]))));
    assert!(empty.contains("at least one"), "{empty}");
    let many = vec!["idled"; 65];
    let too_many = refusal(run(&f.cx, &f.vault, &me, &add(url, Some(many))));
    assert!(too_many.contains("at most 64"), "{too_many}");
    run(&f.cx, &f.vault, &me, &add(url, Some(vec!["idled"; 64]))).unwrap();
}

#[test]
fn a_user_holds_at_most_ten_webhooks() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let (other, _) = enrolled(&f, "local:2001");
    for _ in 0..10 {
        run(&f.cx, &f.vault, &me, &add("https://example.com/h", None)).unwrap();
    }
    let msg = refusal(run(
        &f.cx,
        &f.vault,
        &me,
        &add("https://example.com/h", None),
    ));
    assert!(msg.contains("at most 10"), "{msg}");
    run(&f.cx, &f.vault, &other, &add("https://example.com/h", None)).unwrap();
}

#[test]
fn an_unenrolled_identity_is_refused_with_the_enroll_hint() {
    let f = fixture();
    let me = id("local:2000");
    for cmd in [
        add("https://example.com/h", None),
        UserCmd::WebhookList,
        UserCmd::WebhookRemove { id: 1 },
        UserCmd::WebhookSetupShow { id: 1 },
        finish(&json!(1), None),
    ] {
        let err = run(&f.cx, &f.vault, &me, &cmd).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused);
        assert!(err.error.to_string().contains("has no user"), "{err:?}");
        assert_eq!(err.hints, ["run `moor user enroll`"]);
    }
}

#[test]
fn the_key_at_rest_is_sealed_to_its_user_and_name() {
    let f = fixture();
    let (me, user) = enrolled(&f, "local:2000");
    let (_, stranger) = enrolled(&f, "local:2001");
    let added = run(&f.cx, &f.vault, &me, &add("https://example.com/h", None)).unwrap();
    let shown = run(&f.cx, &f.vault, &me, &show(&added["id"])).unwrap();
    let key = shown["key"].as_str().unwrap();
    let id = added["id"].as_i64().unwrap();
    let blob = f.cx.store().unwrap().webhook_key_blob(id).unwrap();
    assert!(!blob.windows(key.len()).any(|w| w == key.as_bytes()));
    let name = webhook_key_name(id).unwrap();
    let opened = f.vault.open(user, &name, &blob).unwrap();
    assert_eq!(opened.expose(), key.as_bytes());
    assert!(f.vault.open(stranger, &name, &blob).is_err());
    assert!(f
        .vault
        .open(user, &webhook_key_name(id + 1).unwrap(), &blob)
        .is_err());
    let secrets = f.cx.store().unwrap().user_secret_names(user).unwrap();
    assert!(secrets.is_empty(), "{secrets:?}");
}

#[test]
fn a_header_is_authorization_or_an_x_name_moor_does_not_set() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    for ok in [
        "Authorization",
        "authorization",
        "X-Api-Token",
        "x-grok-key",
    ] {
        let added = run(&f.cx, &f.vault, &me, &add_with_header(ok)).unwrap();
        let listed = run(&f.cx, &f.vault, &me, &UserCmd::WebhookList).unwrap();
        let one = listed["webhooks"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            (one["id"].clone(), one["header"].clone()),
            (added["id"].clone(), json!(ok))
        );
        run(
            &f.cx,
            &f.vault,
            &me,
            &UserCmd::WebhookRemove {
                id: added["id"].as_i64().unwrap(),
            },
        )
        .unwrap();
    }
    let long = format!("X-{}", "a".repeat(63));
    for bad in [
        "X-Moor-Signature",
        "x-moor-anything",
        "Content-Type",
        "User-Agent",
        "Host",
        "Content-Length",
        "Bearer",
        "X-Bad Name",
        "X-Bad:Name",
        "X-",
        "",
        long.as_str(),
    ] {
        let msg = refusal(run(&f.cx, &f.vault, &me, &add_with_header(bad)));
        assert!(msg.contains("header"), "{bad}: {msg}");
    }
    let longest = format!("X-{}", "a".repeat(62));
    run(&f.cx, &f.vault, &me, &add_with_header(&longest)).unwrap();
}

#[test]
fn setup_shows_the_key_only_until_it_is_finished() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let added = run(&f.cx, &f.vault, &me, &add("https://example.com/h", None)).unwrap();
    let listed = run(&f.cx, &f.vault, &me, &UserCmd::WebhookList).unwrap();
    assert_eq!(listed["webhooks"][0]["state"], "waiting_for_secret");
    let shown = run(&f.cx, &f.vault, &me, &show(&added["id"])).unwrap();
    assert_eq!(shown["url"], "https://example.com/h");
    assert_eq!(shown["state"], "waiting_for_secret");
    assert!(
        shown["header"].is_null() && shown.get("header").is_some(),
        "{shown}"
    );
    assert_eq!(shown["key"].as_str().map(str::len), Some(64));
    let done = run(&f.cx, &f.vault, &me, &finish(&added["id"], None)).unwrap();
    assert_eq!(done, json!({"id": added["id"], "state": "active"}));
    let after = run(&f.cx, &f.vault, &me, &show(&added["id"])).unwrap();
    assert_eq!(after["state"], "active");
    assert!(after.get("key").is_none(), "{after}");
    let listed = run(&f.cx, &f.vault, &me, &UserCmd::WebhookList).unwrap();
    assert_eq!(listed["webhooks"][0]["state"], "active");
}

#[test]
fn finishing_twice_is_refused_with_how_to_change_the_secret() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let added = run(&f.cx, &f.vault, &me, &add("https://example.com/h", None)).unwrap();
    run(&f.cx, &f.vault, &me, &finish(&added["id"], None)).unwrap();
    let again = refusal(run(&f.cx, &f.vault, &me, &finish(&added["id"], None)));
    assert!(
        again.contains("already set up; remove and add it again"),
        "{again}"
    );
}

#[test]
fn finish_takes_a_header_value_exactly_when_the_webhook_has_a_header() {
    let f = fixture();
    let (me, _) = enrolled(&f, "local:2000");
    let plain = run(&f.cx, &f.vault, &me, &add("https://example.com/h", None)).unwrap();
    let extra = refusal(run(&f.cx, &f.vault, &me, &finish(&plain["id"], Some("t"))));
    assert!(extra.contains("has no header"), "{extra}");
    let headed = run(&f.cx, &f.vault, &me, &add_with_header("Authorization")).unwrap();
    let missing = refusal(run(&f.cx, &f.vault, &me, &finish(&headed["id"], None)));
    assert!(
        missing.contains("needs a value for its `Authorization` header"),
        "{missing}"
    );
    let huge = "a".repeat(4097);
    for bad in ["", "Bearer a\r\nX-Evil: 1", "a\nb", huge.as_str()] {
        let msg = refusal(run(&f.cx, &f.vault, &me, &finish(&headed["id"], Some(bad))));
        assert!(msg.contains("header_value"), "{msg}");
    }
    let shown = run(&f.cx, &f.vault, &me, &show(&headed["id"])).unwrap();
    assert_eq!(
        shown["state"], "waiting_for_secret",
        "a refusal changes nothing"
    );
}

#[test]
fn a_header_value_up_to_4096_bytes_is_sealed_to_its_user_and_name() {
    let f = fixture();
    let (me, user) = enrolled(&f, "local:2000");
    let headed = run(&f.cx, &f.vault, &me, &add_with_header("Authorization")).unwrap();
    let token = format!("Bearer {}", "t".repeat(4089));
    assert_eq!(token.len(), 4096);
    run(&f.cx, &f.vault, &me, &finish(&headed["id"], Some(&token))).unwrap();
    let id = headed["id"].as_i64().unwrap();
    let blob = f.cx.store().unwrap().webhook_header_blob(id).unwrap();
    assert!(!blob.windows(token.len()).any(|w| w == token.as_bytes()));
    let opened = f
        .vault
        .open(user, &webhook_header_name(id).unwrap(), &blob)
        .unwrap();
    assert_eq!(opened.expose(), token.as_bytes());
    assert!(f
        .vault
        .open(user, &webhook_key_name(id).unwrap(), &blob)
        .is_err());
    let listed = run(&f.cx, &f.vault, &me, &UserCmd::WebhookList).unwrap();
    assert!(!listed.to_string().contains("tttt"), "{listed}");
}

#[test]
fn setup_refuses_another_users_or_a_missing_id_alike() {
    let f = fixture();
    let (alice, _) = enrolled(&f, "local:2000");
    let (bob, _) = enrolled(&f, "local:2001");
    let theirs = run(&f.cx, &f.vault, &bob, &add("https://b.example/h", None)).unwrap();
    let missing = json!(9999);
    let msgs: Vec<String> = [
        show(&theirs["id"]),
        show(&missing),
        finish(&theirs["id"], None),
        finish(&missing, None),
    ]
    .iter()
    .map(|cmd| refusal(run(&f.cx, &f.vault, &alice, cmd)))
    .collect();
    assert!(msgs.iter().all(|m| m == "no such webhook"), "{msgs:?}");
    let still = run(&f.cx, &f.vault, &bob, &show(&theirs["id"])).unwrap();
    assert_eq!(still["state"], "waiting_for_secret");
}
