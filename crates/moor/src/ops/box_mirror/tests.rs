//! `box-mirror` against the fake Driver.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxId, BoxMirrorReq, BoxName, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

/// Box `t`, inserted and admitted; `ready` also gives it a live machine.
fn fx_with(driver: FakeDriver, ready: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
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
    let d = if ready {
        let fields = Fields {
            box_provider: Field::Set(provider.clone()),
            ..Fields::default()
        };
        Decision::new(id)
            .lifecycle(BoxLifecycleInput::Admitted)
            .box_input(BoxInput::Requested)
            .set(fields)
    } else {
        Decision::new(id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 })
    };
    store.apply(d).unwrap();
    if ready {
        let got = driver
            .acquire(&AcquireReq {
                provider,
                box_id: id,
                name,
                class: None,
                nonce: "n".to_owned(),
                cache: moor_core::cache::CachePlan::none(),
            })
            .unwrap();
        let d = Decision::new(id)
            .lifecycle(BoxLifecycleInput::Ready {
                setup: SetupOutcome::Ok,
            })
            .session(SessionInput::Started)
            .box_input(BoxInput::Acquired { handle: got.handle });
        store.apply(d).unwrap();
    }
    Fx {
        dir,
        cx,
        driver,
        id,
    }
}

fn fx() -> Fx {
    fx_with(FakeDriver::new(), true)
}

fn req(f: &Fx, id: BoxId) -> BoxMirrorReq {
    BoxMirrorReq {
        id,
        dest: f.dir.path().join("mirror").display().to_string(),
    }
}

fn mirrors(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("mirror_out"))
        .count()
}

#[test]
fn an_unknown_box_id_is_refused() {
    let f = fx();
    let err = run(&f.cx, &req(&f, BoxId::new(f.id.get() + 1))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(err.error.to_string().contains("no open Box"), "{err:?}");
}

#[test]
fn a_closed_box_is_refused() {
    let f = fx();
    let close = Decision::new(f.id).lifecycle(BoxLifecycleInput::Close);
    f.cx.store().unwrap().apply(close).unwrap();
    let err = run(&f.cx, &req(&f, f.id)).unwrap_err();
    assert!(err.error.to_string().contains("no open Box"), "{err:?}");
}

#[test]
fn a_box_without_a_machine_is_refused() {
    let f = fx_with(FakeDriver::new(), false);
    let err = run(&f.cx, &req(&f, f.id)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
}

#[test]
fn a_relative_destination_is_usage() {
    let f = fx();
    let err = run(
        &f.cx,
        &BoxMirrorReq {
            id: f.id,
            dest: "rel".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn a_driver_that_cannot_mirror_is_refused_naming_what_is_missing() {
    let f = fx();
    let err = run(&f.cx, &req(&f, f.id)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("needs mirroring a Box"),
        "{err:?}"
    );
}

#[test]
fn a_mirror_waits_for_no_op_holding_the_box_lock() {
    let f = fx();
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&f.cx, &req(&f, f.id)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("close"), "{err:?}");
    assert_eq!(mirrors(&f), 0);
}
