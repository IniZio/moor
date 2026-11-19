//! Tests for `changes`.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, ListedRepo, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::request::ChangesReq;
use moor_core::seam::driver::{AcquireReq, Driver, Place};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxName, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::testing::cx::{box_repo, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

/// A ready Box `t` on `o/n` and `o/docs`, and the Driver that holds it.
fn ready_with_two_repos(dir: &std::path::Path) -> (Cx, Arc<FakeDriver>) {
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake(dir, driver.clone(), Arc::new(ScriptedProc::new()));
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let mut repo = box_repo();
    repo.others.push(ListedRepo {
        repo: ForgeRepo::new("o", "docs"),
    });
    let new = Decision::insert(NewBox {
        name: name.clone(),
        repo: Some(repo),
        spec: SpecFields::default(),
    })
    .requester(REQUESTER_USER);
    let id = store.apply(new).unwrap().box_id;
    let provider = ProviderId::new("p");
    let fields = HandsFields {
        provider: Field::Set(provider.clone()),
        ..HandsFields::default()
    };
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(admitted).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle: got.handle });
    store.apply(ready).unwrap();
    (cx, driver)
}

#[test]
fn a_live_read_covers_every_repo_in_the_work_place_first_repo_first() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, driver) = ready_with_two_repos(dir.path());
    let req = ChangesReq {
        target: BoxName::parse_allow_bare("t").unwrap().into(),
    };
    let view = run(&cx, &req).unwrap();
    assert!(view.live);
    let repos: Vec<_> = view.repos.iter().map(|r| r.repo.as_str()).collect();
    assert_eq!(repos, ["o/n", "o/docs"]);
    assert_eq!(driver.places("batch"), [Place::Work]);
}
