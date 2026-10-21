//! What a start does near another user's Boxes: it binds none of their worktrees and its
//! reply names none of them.

use std::path::Path;

use moor_core::seam::surface::Surface;
use moor_core::user::{Identity, UserId, Viewer};
use moor_core::{BoxId, ErrorKind, StartReq};

use super::{box_name, req, world, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::testing::cx::put_secret;
use crate::testing::driver::FakeDriver;
use crate::vault::Vault;

/// `cx` acting as local uid 2000, enrolled with the secrets a start needs.
fn second_user(cx: Cx) -> (Cx, UserId) {
    let other = Identity::local(2000);
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(&other).unwrap();
    let vault = Vault::load_or_create(cx.vault_key().unwrap()).unwrap();
    for name in ["claude", "github"] {
        put_secret(store, &vault, user, name, &format!("{name}-of-2000"));
    }
    (cx.with_requester(other), user)
}

#[test]
fn a_worktree_an_open_box_works_in_cannot_be_bound_by_another_start() {
    let mut w: World = world(FakeDriver::new());
    let made = w
        .surface
        .create_worktree(Path::new("/repo"), &box_name("pre"), None)
        .unwrap();
    let path = made.path.display().to_string();
    let bound = |name: &str| StartReq {
        worktree: Some(path.clone()),
        ..req(name)
    };
    run(&w.cx, &bound("box-a")).expect("the first start binds the worktree");
    let refused = |cx: &Cx| {
        let e = run(cx, &bound("box-b")).expect_err("a second Box binds the worktree");
        assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
        let want = format!("{path} is the worktree of an open Box");
        assert_eq!(e.error.to_string(), want);
    };
    refused(&w.cx);
    w.cx = second_user(w.cx).0;
    refused(&w.cx);
    let store = w.cx.store().unwrap();
    assert!(store
        .open_box_by_name(&box_name("box-b"))
        .unwrap()
        .is_none());
}

#[test]
fn a_start_hides_the_settle_warning_of_a_box_its_caller_may_not_see() {
    let mut w = world(FakeDriver::new());
    let bad = BoxId::new(run(&w.cx, &req("bad")).unwrap().boxed.id);
    let raw = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (box, argv, sync, pid, started_at) VALUES (?, 'not json', 'none', 1, 1)",
        [bad.get()],
    )
    .unwrap();
    let (cx, user) = second_user(w.cx);
    w.cx = cx.with_viewer(Viewer::User(user));
    run(&w.cx, &req("theirs")).unwrap();
    let warnings = w.cx.take_warnings();
    let named = format!("Box {}", bad.get());
    assert!(
        !warnings
            .iter()
            .any(|x| x.code == "settle_failed" || x.message.contains(&named)),
        "{warnings:?}"
    );
    w.cx = w.cx.with_viewer(Viewer::Operator);
    run(&w.cx, &req("operators")).unwrap();
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(codes.contains(&"settle_failed".to_owned()), "{codes:?}");
}
