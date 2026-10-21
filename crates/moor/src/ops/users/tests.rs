//! Tests for the user requests run as a checked identity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::request::{AsReq, UserCmd};
use moor_core::user::Identity;
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
        Path::new("/home/me/proj"),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(PathBuf::from("/s"))),
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
    let me = id("local:1000");
    let before = run(&f.cx, &f.vault, &me, &UserCmd::Whoami).unwrap();
    assert_eq!(
        before,
        json!({"identity": "local:1000", "user": null, "identities": []})
    );
    let store = f.cx.store().unwrap();
    let (user, _) = store.enroll(&me).unwrap();
    let after = run(&f.cx, &f.vault, &me, &UserCmd::Whoami).unwrap();
    assert_eq!(
        after,
        json!({"identity": "local:1000", "user": user.get(), "identities": ["local:1000"]})
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
