//! A streamed `run` acts with the grant its request was checked with, not the operator's.
use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::request::Grant;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, Viewer};
use moor_core::{BoxName, ProviderId, Request, RunReq};

use super::super::conn::Acting;
use super::super::progress::{respond_with_progress, INTERVAL};
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;

#[test]
fn a_streamed_run_acts_on_the_box_its_grant_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().unwrap()),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let name = BoxName::parse_allow_bare("a").unwrap();
    let open = |who: u32| {
        let (user, _) = store.enroll(&Identity::local(who)).unwrap();
        let new = NewBox {
            name: name.clone(),
            repo: "r".into(),
        };
        let id = store
            .apply(Decision::insert(new).requester(user))
            .unwrap()
            .box_id;
        (user, id)
    };
    let (mine, checked) = open(1000);
    let close = Decision::new(checked)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    store.apply(close).unwrap();
    let (_, theirs) = open(2000);
    let starting = Decision::new(theirs)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(starting).unwrap();
    let acting = Acting {
        who: None,
        grant: Grant {
            viewer: Viewer::User(mine),
            pins: [(name.clone(), checked)].into(),
        },
        dir: None,
    };
    let run = Request::Run(RunReq {
        box_name: name,
        record: None,
        argv: vec!["true".into()],
        inherit_output: false,
        trust_moor_yaml: false,
    });
    let mut wire = Vec::new();
    let response = respond_with_progress(&env, acting, None, "1", run, INTERVAL, &mut wire);
    let error = response.error.expect("refused");
    assert_eq!(error.message, "a has no box", "{error:?}");
}
