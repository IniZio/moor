//! Tests for `cp`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::{BoxHandle, BoxId, BoxName, CpReq, ErrorKind, ProviderId};

use super::run;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const CWD: &str = "/home/me/proj";

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
        Path::new(CWD),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    );
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
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

/// The `cp` request a CLI in [`CWD`] sends: `dest` made absolute against it.
fn req(src: &str, dest: &str) -> CpReq {
    CpReq {
        box_name: BoxName::parse_allow_bare("t").unwrap(),
        src: src.into(),
        dest: Path::new(CWD).join(dest).display().to_string(),
    }
}

/// Kinds of the events recorded for the Box.
fn kinds(f: &Fx) -> Vec<String> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter().map(|e| e.kind).collect()
}

/// A destination under the fixture's own directory.
fn dest(f: &Fx) -> String {
    f.dir.path().join("out").display().to_string()
}

/// The first `copy_out` the Driver saw.
fn first_copy(f: &Fx) -> (BoxHandle, BoxPath, PathBuf) {
    f.driver.copies().into_iter().next().unwrap()
}

fn copied(f: &Fx) -> usize {
    kinds(f).iter().filter(|k| *k == "copied").count()
}

#[test]
fn relative_source_resolves_against_the_box_workdir() {
    let f = fx();
    run(&f.cx, &req("out/a.txt", "a.txt")).unwrap();
    assert_eq!(f.driver.copies().len(), 1);
    assert_eq!(first_copy(&f).1.as_str(), "/work/cbx_1/repo/out/a.txt");
}

#[test]
fn absolute_source_is_used_as_given() {
    let f = fx();
    run(&f.cx, &req("/var/log/x", "x")).unwrap();
    assert_eq!(first_copy(&f).1.as_str(), "/var/log/x");
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("workdir")));
}

#[test]
fn undeterminable_workdir_makes_no_copy() {
    let f = fx();
    f.driver.forget(&BoxHandle::new("box-1"));
    run(&f.cx, &req("a", "a")).unwrap_err();
    assert!(f.driver.copies().is_empty());
}

#[test]
fn source_climbing_out_of_the_workdir_is_usage() {
    let f = fx();
    let err = run(&f.cx, &req("../x", "x")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn a_relative_destination_is_usage_without_a_copy() {
    let f = fx();
    let relative = CpReq {
        dest: "sub/a.txt".into(),
        ..req("a", "a")
    };
    let err = run(&f.cx, &relative).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(
        err.error.to_string().contains("not an absolute path"),
        "{err:?}"
    );
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn destination_stays_under_the_callers_cwd() {
    let f = fx();
    run(&f.cx, &req("a", "out/a.txt")).unwrap();
    let (_, _, to) = first_copy(&f);
    assert!(to.starts_with(CWD));
}

#[test]
fn absolute_destination_is_kept() {
    let f = fx();
    let to = dest(&f);
    run(&f.cx, &req("a", &to)).unwrap();
    assert_eq!(first_copy(&f).2, Path::new(&to));
}

#[test]
fn unknown_box_is_refused_without_a_copy() {
    let f = fx();
    let mut r = req("a", "a");
    r.box_name = BoxName::parse_allow_bare("nope").unwrap();
    run(&f.cx, &r).unwrap_err();
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn blank_source_is_usage_without_a_copy() {
    let f = fx();
    let err = run(&f.cx, &req("  ", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn gone_box_is_refused_without_a_copy() {
    let f = fx();
    let d = Decision::new(f.id).box_input(BoxInput::DriverMissing);
    f.cx.store().unwrap().apply(d).unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn box_waiting_for_a_box_is_refused_without_a_copy() {
    let f = fx_with(FakeDriver::new(), false);
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn success_records_one_copied_event_and_confirms() {
    let f = fx();
    let v = run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert!(v.copied);
    assert_eq!(v.to, dest(&f));
    assert!(!v.confirmation.is_empty());
    assert_eq!(copied(&f), 1);
}

#[test]
fn failed_copy_still_records_one_copied_event() {
    let f = fx_with(
        FakeDriver::new().fail_copy_out(DriverError::Quota("q".into())),
        true,
    );
    let err = run(&f.cx, &req("a", &dest(&f))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(copied(&f), 1);
}

#[test]
fn copy_refuses_while_another_op_holds_the_box_lock() {
    let f = fx();
    let _held = BoxLock::acquire(
        f.cx.clock(),
        f.cx.state_dir().unwrap(),
        f.id,
        "close",
        Duration::ZERO,
    )
    .unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn copy_never_syncs() {
    let f = fx();
    run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("exec")));
}

#[test]
fn closed_box_with_a_release_failed_box_is_refused_without_a_copy() {
    let f = fx();
    let store = f.cx.store().unwrap();
    let d = Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() });
    store.apply(d).unwrap();
    store
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End),
        )
        .unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn box_that_is_not_ready_is_refused_without_a_copy() {
    let f = fx();
    let d = Decision::new(f.id).box_input(BoxInput::ReleaseErr { cause: "x".into() });
    f.cx.store().unwrap().apply(d).unwrap();
    assert_eq!(
        run(&f.cx, &req("a", "a")).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert!(f.driver.copies().is_empty());
}

fn sleep(f: &Fx) {
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::Sleep { auto: false })
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
}

fn wake_calls(f: &Fx) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("wake"))
        .count()
}

fn box_state(f: &Fx) -> BoxState {
    f.cx.store()
        .unwrap()
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .boxed
        .unwrap()
        .state
}

#[test]
fn asleep_box_is_woken_before_the_copy() {
    let f = fx();
    sleep(&f);
    run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert_eq!(wake_calls(&f), 1);
    assert_eq!(f.driver.copies().len(), 1);
    assert_eq!(box_state(&f), BoxState::Ready);
    let k = kinds(&f);
    assert!(k.contains(&"woke".to_owned()));
    assert_eq!(copied(&f), 1);
}

#[test]
fn failed_wake_leaves_the_box_asleep_and_makes_no_copy() {
    let f = fx_with(FakeDriver::new().fail_wake(DriverError::NotFound), true);
    sleep(&f);
    run(&f.cx, &req("a", &dest(&f))).unwrap_err();
    assert_eq!(wake_calls(&f), 1);
    assert_eq!(box_state(&f), BoxState::Asleep);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn gone_asleep_box_is_refused_without_a_wake_or_copy() {
    let f = fx();
    sleep(&f);
    let d = Decision::new(f.id).box_input(BoxInput::DriverMissing);
    f.cx.store().unwrap().apply(d).unwrap();
    let err = run(&f.cx, &req("a", "a")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(wake_calls(&f), 0);
    assert!(f.driver.copies().is_empty());
    assert_eq!(copied(&f), 0);
}

#[test]
fn ready_box_is_copied_without_a_wake() {
    let f = fx();
    run(&f.cx, &req("a", &dest(&f))).unwrap();
    assert_eq!(wake_calls(&f), 0);
    assert_eq!(f.driver.copies().len(), 1);
}

#[test]
fn size_does_not_follow_directory_links() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f"), b"abc").unwrap();
    std::os::unix::fs::symlink(".", dir.path().join("loop")).unwrap();
    std::os::unix::fs::symlink("/", dir.path().join("root")).unwrap();
    let total = super::size(dir.path());
    assert!((3..1000).contains(&total), "{total}");
}

#[test]
fn leading_current_dir_is_dropped_from_the_source() {
    let f = fx();
    run(&f.cx, &req("./rel.txt", "rel.txt")).unwrap();
    assert_eq!(first_copy(&f).1.as_str(), "/work/cbx_1/repo/rel.txt");
}

#[test]
fn interior_current_dir_is_dropped_from_the_source() {
    let f = fx();
    run(&f.cx, &req("./a/./b.txt", "b.txt")).unwrap();
    assert_eq!(first_copy(&f).1.as_str(), "/work/cbx_1/repo/a/b.txt");
}

#[test]
fn parent_dir_after_current_dir_is_still_a_usage_error() {
    let f = fx();
    let e = run(&f.cx, &req("./../x", "x")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
    assert!(f.driver.copies().is_empty());
}

#[test]
fn source_of_only_current_dir_is_a_usage_error() {
    let f = fx();
    let e = run(&f.cx, &req(".", "x")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Usage);
}
