//! Tests for the user requests run as a checked identity.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::sync::Arc;

use moor_core::request::{AsReq, SecretValue, UserCmd};
use moor_core::user::{Email, Identity, SecretName};
use moor_core::{ErrorKind, Request};
use serde_json::json;

use super::run;
use crate::cx::Cx;
use crate::ops::dispatch;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use crate::vault::Vault;

pub(super) struct Fixture {
    pub(super) _dir: tempfile::TempDir,
    pub(super) cx: Cx,
    pub(super) vault: Vault,
}

pub(super) fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    Fixture {
        _dir: dir,
        cx,
        vault,
    }
}

pub(super) fn id(s: &str) -> Identity {
    Identity::parse(s).unwrap()
}

#[test]
fn whoami_names_the_identity_before_and_after_it_has_a_user() {
    let f = fixture();
    let me = id("local:2000");
    let before = run(&f.cx, &f.vault, &me, &UserCmd::Whoami).unwrap();
    assert_eq!(
        before,
        json!({"identity": "local:2000", "user": null, "identities": []})
    );
    let store = f.cx.store().unwrap();
    let (user, _) = store.enroll(&me, None).unwrap();
    let after = run(&f.cx, &f.vault, &me, &UserCmd::Whoami).unwrap();
    assert_eq!(
        after,
        json!({"identity": "local:2000", "user": user.get(), "identities": ["local:2000"]})
    );
}

#[test]
fn only_the_controller_socket_serves_user_requests() {
    let f = fixture();
    let err = dispatch(&f.cx, Request::Whoami(AsReq { identity: None })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(
        err.error.to_string().contains("controller socket"),
        "{err:?}"
    );
}

fn secret_set(name: &str, value: &str) -> UserCmd {
    UserCmd::SecretSet {
        name: SecretName::parse(name).unwrap(),
        value: SecretValue::new(value.into()),
    }
}

#[test]
fn enroll_is_idempotent_and_a_user_is_needed_for_secrets() {
    let f = fixture();
    let me = id("local:2000");
    let err = run(&f.cx, &f.vault, &me, &secret_set("claude", "v")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("has no user"), "{err:?}");
    let first = run(&f.cx, &f.vault, &me, &UserCmd::Enroll { email: None }).unwrap();
    assert_eq!(first["created"], true);
    let again = run(&f.cx, &f.vault, &me, &UserCmd::Enroll { email: None }).unwrap();
    assert_eq!(
        (again["created"].clone(), again["user"].clone()),
        (json!(false), first["user"].clone())
    );
}

#[test]
fn secrets_set_list_and_delete_by_name_only() {
    let f = fixture();
    let me = id("local:2000");
    run(&f.cx, &f.vault, &me, &UserCmd::Enroll { email: None }).unwrap();
    let set = run(&f.cx, &f.vault, &me, &secret_set("claude", "tok-1")).unwrap();
    assert_eq!(set, json!({"name": "claude", "changed": true}));
    let names = run(&f.cx, &f.vault, &me, &UserCmd::SecretNames).unwrap();
    assert_eq!(names, json!({"names": ["claude"]}));
    let delete = UserCmd::SecretDelete {
        name: SecretName::parse("claude").unwrap(),
    };
    let gone = run(&f.cx, &f.vault, &me, &delete).unwrap();
    assert_eq!(gone, json!({"name": "claude", "changed": true}));
    let again = run(&f.cx, &f.vault, &me, &delete).unwrap();
    assert_eq!(again["changed"], false);
    let err = run(&f.cx, &f.vault, &me, &secret_set("claude", "")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn identities_with_one_email_see_the_same_secrets() {
    let f = fixture();
    let email = Email::parse("alice@example.com").unwrap();
    let enroll = UserCmd::Enroll { email: Some(email) };
    let slack = id("slack:T1:U1");
    let first = run(&f.cx, &f.vault, &slack, &enroll).unwrap();
    run(&f.cx, &f.vault, &slack, &secret_set("claude", "tok-1")).unwrap();
    let authgear = id("authgear:sub-1");
    let second = run(&f.cx, &f.vault, &authgear, &enroll).unwrap();
    assert_eq!(second["user"], first["user"]);
    assert_eq!(second["created"], false);
    let names = run(&f.cx, &f.vault, &authgear, &UserCmd::SecretNames).unwrap();
    assert_eq!(names, json!({"names": ["claude"]}));
    let who = run(&f.cx, &f.vault, &authgear, &UserCmd::Whoami).unwrap();
    assert_eq!(who["identities"], json!(["authgear:sub-1", "slack:T1:U1"]));
    let third = run(
        &f.cx,
        &f.vault,
        &id("web:bob"),
        &UserCmd::Enroll { email: None },
    )
    .unwrap();
    assert_ne!(third["user"], first["user"]);
}
