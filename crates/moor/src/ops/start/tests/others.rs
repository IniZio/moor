//! What a start does near another user's Boxes: its reply names none of them.

use moor_core::user::{Identity, UserId, Viewer};
use moor_core::BoxId;

use super::{req, world};
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
fn a_start_hides_the_settle_warning_of_a_box_its_caller_may_not_see() {
    let mut w = world(FakeDriver::new());
    let bad = BoxId::new(run(&w.cx, &req("bad")).unwrap().boxed.id);
    let raw = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (box, argv, pid, started_at) VALUES (?, 'not json', 1, 1)",
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

#[test]
fn a_start_shows_the_settle_warning_of_its_callers_own_box() {
    let mut w = world(FakeDriver::new());
    let (cx, user) = second_user(w.cx);
    w.cx = cx.with_viewer(Viewer::User(user));
    let mine = BoxId::new(run(&w.cx, &req("mine")).unwrap().boxed.id);
    let raw = rusqlite::Connection::open(w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO runs (box, argv, pid, started_at) VALUES (?, 'not json', 1, 1)",
        [mine.get()],
    )
    .unwrap();
    w.cx.take_warnings();
    run(&w.cx, &req("next")).unwrap();
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(codes.contains(&"settle_failed".to_owned()), "{codes:?}");
}
