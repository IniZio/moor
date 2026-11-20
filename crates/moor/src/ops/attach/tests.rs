//! `attach` argv against the fake Driver.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, Place};
use moor_core::{AttachReq, BoxId, BoxName, ProviderId};

use super::argv;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;

/// A Cx holding Box `t` with a live machine, the driver's connect argv for it, and the work
/// tree the driver reports for it.
fn ready_box(dir: &std::path::Path) -> (Cx, Vec<String>, String, Arc<FakeDriver>) {
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake(dir, driver.clone(), Arc::new(ScriptedProc::new()));
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = HandsFields {
        provider: Field::Set(provider.clone()),
        ..HandsFields::default()
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .hands_input(HandsInc::FIRST, HandsInput::Requested)
                .hands(HandsInc::FIRST, fields),
        )
        .unwrap();
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
    let workdir = driver.workdir(&got.handle).unwrap().as_str().to_owned();
    let connect = driver.connect_argv(&got.handle, Place::Agent).unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started)
                .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle: got.handle }),
        )
        .unwrap();
    (cx, connect, workdir, driver)
}

fn req(shell: bool) -> AttachReq {
    serde_json::from_value(serde_json::json!({
        "box": "t", "cols": 80, "rows": 24, "shell": shell
    }))
    .unwrap()
}

#[test]
fn a_shell_attach_runs_a_login_shell_in_the_work_tree_over_connect() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, connect, workdir, driver) = ready_box(dir.path());
    let got = argv(&cx, &req(true)).unwrap();
    let (last, head) = got.split_last().unwrap();
    assert_eq!(head, connect.as_slice());
    assert_eq!(driver.places("connect_argv").last(), Some(&Place::Work));
    assert_eq!(
        last,
        &format!("cd '{workdir}' && exec \"${{SHELL:-bash}}\" -l")
    );
}

#[test]
fn an_agent_attach_never_opens_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _, driver) = ready_box(dir.path());
    let got = argv(&cx, &req(false))
        .map(|a| a.join(" "))
        .unwrap_or_default();
    assert!(!got.contains("SHELL"), "{got}");
    assert_eq!(driver.places("connect_argv").last(), Some(&Place::Agent));
}

#[test]
fn an_agent_attach_relays_through_herdr_core() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _, driver) = ready_box(dir.path());
    let cx = cx.with_engine(Arc::new(FakeEngine::new()));
    cx.store()
        .unwrap()
        .begin_launch(BoxId::new(1), "s1")
        .unwrap();
    assert_eq!(argv(&cx, &req(false)).unwrap().pop().unwrap(), "relay");
    assert_eq!(driver.places("connect_argv").last(), Some(&Place::Agent));
}

#[test]
fn an_agent_attach_to_a_box_another_holder_kept_says_to_recreate_it() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _, _) = ready_box(dir.path());
    let cx = cx.with_engine(Arc::new(FakeEngine::new()));
    let store = cx.store().unwrap();
    let id = BoxId::new(1);
    store.begin_launch(id, "s1").unwrap();
    store.set_holders(id, Some("tmux")).unwrap();
    let e = argv(&cx, &req(false)).unwrap_err().error.to_string();
    assert!(e.contains("close the Box and recreate it"), "{e}");
}

#[test]
fn opening_a_terminal_leaves_a_record_that_a_person_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _, _) = ready_box(dir.path());
    argv(&cx, &req(true)).unwrap();
    let store = cx.store().unwrap();
    let opened = store.events(BoxId::new(1), 100).unwrap();
    let opened: Vec<_> = opened
        .iter()
        .filter(|e| e.kind == "terminal_opened")
        .collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened.first().and_then(|e| e.payload.get("shell")),
        Some(&serde_json::json!(true))
    );
    assert!(store
        .last_event_at(BoxId::new(1), &["terminal_opened"])
        .unwrap()
        .is_some());
}

#[test]
fn a_refused_terminal_leaves_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _, _) = ready_box(dir.path());
    let mut gone = req(true);
    gone.target = BoxName::parse_allow_bare("nope").unwrap().into();
    assert!(argv(&cx, &gone).is_err());
    let events = cx.store().unwrap().events(BoxId::new(1), 100).unwrap();
    assert!(events.iter().all(|e| e.kind != "terminal_opened"));
}
