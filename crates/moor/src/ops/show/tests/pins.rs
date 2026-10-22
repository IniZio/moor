//! The check pins each Box name to the Box it checked, so a handler acts on that Box even after
//! another of the same name opens.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::request::BoxReq;
use moor_core::user::{Caller, Identity};
use moor_core::{BoxId, BoxName, BoxTarget, Request, ShowReq, StartReq};

use super::{cx, req};
use crate::cx::Cx;
use crate::ops::authz::authorize;
use crate::ops::show::run;
use crate::testing::driver::FakeDriver;

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

/// Inserts open Box `n` acting for the user of `who`.
fn add_for(cx: &Cx, n: &str, who: &Identity) -> BoxId {
    let store = cx.store().unwrap();
    let (user, _) = store.enroll(who).unwrap();
    let new = NewBox {
        name: name(n),
        repo: None,
    };
    store
        .apply(Decision::insert(new).requester(user))
        .unwrap()
        .box_id
}

fn close(cx: &Cx, id: BoxId) {
    let close = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    cx.store().unwrap().apply(close).unwrap();
}

#[test]
fn the_check_pins_a_name_and_the_handler_acts_on_the_pinned_box() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let setup = cx(dir.path(), &driver);
    let mine = add_for(&setup, "a", &Identity::local(1000));
    let show = |target: &str| ShowReq {
        target: Some(BoxTarget::Name(name(target))),
        ..req(None, None)
    };
    let caller = Ok(Caller::Local(1000));
    let grant = authorize(&setup, &caller, None, &Request::Show(show("a"))).unwrap();
    assert_eq!(grant.pins.get(&name("a")), Some(&mine));
    close(&setup, mine);
    let theirs = add_for(&setup, "a", &Identity::local(2000));
    let pinned = cx(dir.path(), &driver).with_grant(grant);
    let shown = run(&pinned, &show("a")).unwrap().boxed.unwrap();
    assert_eq!(shown.id, mine.get(), "not the newer Box {}", theirs.get());
}

#[test]
fn the_check_pins_the_open_box_a_start_attaches_to() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let setup = cx(dir.path(), &driver);
    let mine = add_for(&setup, "a", &Identity::local(1000));
    let caller = Ok(Caller::Local(1000));
    let start = Request::Start(StartReq {
        name: name("a"),
        goal: None,
        issue: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        repo: None,
        parent: None,
        identity: None,
    });
    let grant = authorize(&setup, &caller, None, &start).unwrap();
    assert_eq!(grant.pins.get(&name("a")), Some(&mine));
    let sleep = Request::Sleep(BoxReq {
        box_name: name("a"),
    });
    let grant = authorize(&setup, &caller, None, &sleep).unwrap();
    assert_eq!(grant.pins.get(&name("a")), Some(&mine));
}
