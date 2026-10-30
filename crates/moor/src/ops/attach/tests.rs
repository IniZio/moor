//! `attach` argv against the fake Driver.

use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{AttachReq, BoxName, ProviderId};

use super::argv;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

/// A Cx holding Box `t` with a live machine, the driver's connect argv for it, and the work
/// tree the driver reports for it.
fn ready_box(dir: &std::path::Path) -> (Cx, Vec<String>, String) {
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake(
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        ..Fields::default()
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(fields),
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
    let connect = driver.connect_argv(&got.handle).unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started)
                .box_input(BoxInput::Acquired { handle: got.handle }),
        )
        .unwrap();
    (cx, connect, workdir)
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
    let (cx, connect, workdir) = ready_box(dir.path());
    let got = argv(&cx, &req(true)).unwrap();
    let (last, head) = got.split_last().unwrap();
    assert_eq!(head, connect.as_slice());
    assert_eq!(
        last,
        &format!("cd '{workdir}' && exec \"${{SHELL:-bash}}\" -l")
    );
}

#[test]
fn an_agent_attach_never_opens_a_shell() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _, _) = ready_box(dir.path());
    let got = argv(&cx, &req(false))
        .map(|a| a.join(" "))
        .unwrap_or_default();
    assert!(!got.contains("SHELL"), "{got}");
}
