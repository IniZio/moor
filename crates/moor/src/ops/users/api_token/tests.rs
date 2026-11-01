#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test assertions index JSON values"
)]

use std::sync::Arc;
use std::time::Duration;

use moor_core::request::{ApiTokenCmd, SecretValue, UserCmd};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::user::{Caller, Email, Identity};
use moor_core::ErrorKind;
use serde_json::{json, Value};

use super::{resolve, FRONT_ONLY, UNKNOWN};
use crate::error::CommandError;
use crate::ops::users::run;
use crate::ops::users::tests::{fixture, id, Fixture};
use crate::store::Store;
use crate::testing::clock::FakeClock;

fn create(name: &str, scopes: &[&str]) -> UserCmd {
    UserCmd::ApiToken(ApiTokenCmd::Create {
        name: name.to_owned(),
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
    })
}

fn list(f: &Fixture, who: &Identity) -> Value {
    let cmd = UserCmd::ApiToken(ApiTokenCmd::List);
    run(&f.cx, &f.vault, who, &cmd).unwrap()["tokens"].clone()
}

fn revoke(id: &Value) -> UserCmd {
    UserCmd::ApiToken(ApiTokenCmd::Revoke {
        id: id.as_i64().unwrap(),
    })
}

fn refusal(r: Result<Value, CommandError>) -> String {
    let err = r.unwrap_err();
    assert!(
        matches!(err.kind, ErrorKind::Refused | ErrorKind::Usage),
        "{err:?}"
    );
    err.error.to_string()
}

/// `who` enrolled with `email`.
fn enrolled(f: &Fixture, who: &str, email: &str) -> Identity {
    let me = id(who);
    let email = Email::parse(email).unwrap();
    f.cx.store().unwrap().enroll(&me, Some(&email)).unwrap();
    me
}

fn front(ns: &str) -> Caller {
    Caller::Front(ns.to_owned())
}

fn resolved(f: &Fixture, ns: &str, token: &str) -> Result<Value, CommandError> {
    let token = SecretValue::new(token.as_bytes().to_vec());
    resolve(f.cx.store().unwrap(), &Ok(front(ns)), &token)
}

#[test]
fn a_new_token_is_shown_once_and_listed_by_its_last_four_characters() {
    let f = fixture();
    let me = enrolled(&f, "authgear:alice", "alice@example.com");
    let made = run(&f.cx, &f.vault, &me, &create("grok bot", &["moor:read"])).unwrap();
    let token = made["token"].as_str().unwrap();
    let body = token.strip_prefix("moor_").unwrap();
    assert_eq!(body.len(), 43, "{token}");
    assert!(body
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    let listed = list(&f, &me);
    assert_eq!(
        listed,
        json!([{
            "id": made["id"],
            "name": "grok bot",
            "scopes": ["moor:read"],
            "hint": token.get(token.len() - 4..).unwrap(),
            "created_at": 1000,
            "last_used_at": null,
        }])
    );
    assert!(
        !listed.to_string().contains(body),
        "the text is never listed"
    );
}

#[test]
fn names_scopes_and_the_per_user_cap_are_checked() {
    let f = fixture();
    let me = enrolled(&f, "authgear:alice", "alice@example.com");
    let ask = |cmd: UserCmd| refusal(run(&f.cx, &f.vault, &me, &cmd));
    assert!(ask(create("", &["moor:read"])).contains("1 to 64"));
    assert!(ask(create(&"n".repeat(65), &["moor:read"])).contains("1 to 64"));
    assert!(ask(create("a\nb", &["moor:read"])).contains("printable"));
    assert!(ask(create("bot", &[])).contains("at least one"));
    assert!(ask(create("bot", &["moor:admin"])).contains("`moor:admin`"));
    let both = run(
        &f.cx,
        &f.vault,
        &me,
        &create("bot", &["moor:write", "moor:read"]),
    );
    assert!(both.is_ok(), "{both:?}");
    assert_eq!(
        list(&f, &me)[0]["scopes"],
        json!(["moor:read", "moor:write"])
    );
    assert!(ask(create("bot", &["moor:read"])).contains("named `bot` exists"));
}

#[test]
fn a_user_holds_at_most_20_tokens() {
    let f = fixture();
    let me = enrolled(&f, "authgear:alice", "alice@example.com");
    let ask = |cmd: UserCmd| refusal(run(&f.cx, &f.vault, &me, &cmd));
    for n in 0..20 {
        run(
            &f.cx,
            &f.vault,
            &me,
            &create(&format!("b{n}"), &["moor:read"]),
        )
        .unwrap();
    }
    assert!(ask(create("one more", &["moor:read"])).contains("at most 20"));
}

#[test]
fn revoking_another_users_or_a_missing_token_is_the_same_refusal() {
    let f = fixture();
    let alice = enrolled(&f, "authgear:alice", "alice@example.com");
    let bob = enrolled(&f, "authgear:bob", "bob@example.com");
    let made = run(&f.cx, &f.vault, &alice, &create("bot", &["moor:read"])).unwrap();
    assert_eq!(
        refusal(run(&f.cx, &f.vault, &bob, &revoke(&made["id"]))),
        "no such token"
    );
    assert_eq!(
        refusal(run(&f.cx, &f.vault, &alice, &revoke(&json!(999)))),
        "no such token"
    );
    let gone = run(&f.cx, &f.vault, &alice, &revoke(&made["id"])).unwrap();
    assert_eq!(gone, json!({"id": made["id"]}));
    assert_eq!(list(&f, &alice), json!([]));
    let token = made["token"].as_str().unwrap();
    assert_eq!(refusal(resolved(&f, "authgear", token)), UNKNOWN);
}

#[test]
fn a_front_learns_the_owner_in_its_own_namespace_and_nothing_else() {
    let f = fixture();
    let alice = enrolled(&f, "authgear:alice", "alice@example.com");
    let made = run(&f.cx, &f.vault, &alice, &create("bot", &["moor:read"])).unwrap();
    let token = made["token"].as_str().unwrap();
    assert_eq!(
        resolved(&f, "authgear", token).unwrap(),
        json!({"identity": "authgear:alice", "email": "alice@example.com", "scopes": ["moor:read"]})
    );
    assert_eq!(
        refusal(resolved(&f, "slack", token)),
        UNKNOWN,
        "no slack identity"
    );
    assert_eq!(refusal(resolved(&f, "authgear", "moor_nope")), UNKNOWN);
    let store = f.cx.store().unwrap();
    let presented = SecretValue::new(token.as_bytes().to_vec());
    for caller in [
        Ok(Caller::Operator(0)),
        Ok(Caller::Runner),
        Err("x".to_owned()),
    ] {
        let got = resolve(store, &caller, &presented).unwrap_err();
        assert_eq!(got.error.to_string(), FRONT_ONLY);
    }
}

#[test]
fn last_used_moves_at_most_once_a_minute() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let store = Store::open(&dir.path().join("moor.db"), Arc::clone(&clock) as _).unwrap();
    let alice = id("authgear:alice");
    let (user, _) = store.enroll(&alice, None).unwrap();
    let made = super::run(
        &store,
        user,
        &ApiTokenCmd::Create {
            name: "bot".to_owned(),
            scopes: vec!["moor:read".to_owned()],
        },
    )
    .unwrap();
    let token = SecretValue::new(made["token"].as_str().unwrap().as_bytes().to_vec());
    let use_once = || resolve(&store, &Ok(front("authgear")), &token).unwrap();
    let last_used = || store.api_tokens(user).unwrap()[0].last_used_at;
    assert_eq!(use_once()["email"], json!(null));
    assert_eq!(last_used(), Some(1000));
    clock.sleep(Duration::from_secs(59));
    use_once();
    assert_eq!(last_used(), Some(1000), "within the minute");
    clock.sleep(Duration::from_secs(1));
    use_once();
    assert_eq!(last_used(), Some(61_000));
}
