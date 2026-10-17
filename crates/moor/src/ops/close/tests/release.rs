//! Close releases the Box through the one release op: the lease rows decide Released.

use std::sync::Arc;

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::proc::Tool;
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxHandle, BoxPath, CloseReq, ErrorKind, ProviderId, TaskName};

use super::{box_state, fx_with, proc, req, run, Fx, HEAD};
use crate::cx::Cx;
use crate::ops::common::release::steps::{plan, script, MacTarget};
use crate::ops::common::release::testkit::framed;
use crate::store::lease::LeaseKind;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;
use crate::testing::surface::FakeSurface;

/// A Ready task whose Box has a root and a compose lease; the probes and the reports index exit 0 and compose-down exits `code`.
fn leased(code: i32) -> Fx {
    let driver =
        quiet(3).exec_results([0, 0, 0, code].map(|code| Ok(ExecOutcome::Exited { code })));
    let f = fx_with(driver, FakeSurface::new("/s"), 0);
    let store = f.cx.store().unwrap();
    store.plan_lease(f.id, LeaseKind::BoxRoot, "/root").unwrap();
    store.plan_lease(f.id, LeaseKind::Compose, "proj").unwrap();
    f
}

/// A driver whose release scripts print nothing of their own, however many run.
fn quiet(probes: usize) -> FakeDriver {
    printing(probes, &[])
}

/// A driver whose release scripts print `outs` (script index, text) between their markers.
/// The first `probes` batched runs are the probes before the release and print nothing.
fn printing(probes: usize, outs: &[(u32, &str)]) -> FakeDriver {
    let release: Vec<String> = (0..16)
        .map(|i| framed(i, outs.iter().find(|o| o.0 == i).map_or("", |o| o.1)))
        .collect();
    let lead = vec![""; probes];
    FakeDriver::new().run_stdouts(lead.into_iter().chain(release.iter().map(String::as_str)))
}

fn lease_count(f: &Fx) -> usize {
    f.cx.store().unwrap().leases(f.id).unwrap().len()
}

fn released(f: &Fx) -> bool {
    f.driver.calls().iter().any(|c| c.starts_with("release"))
}

#[test]
fn a_clean_release_drops_the_lease_rows_and_deletes_the_box() {
    let f = leased(0);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert_eq!(lease_count(&f), 0);
    assert!(released(&f));
}

#[test]
fn a_failed_release_step_keeps_the_lease_rows_and_the_task_open_on_a_mac_host() {
    let f = mac_leased(1);
    let e = try_close_envchk2(&f).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("the task stays open"));
    assert_eq!(lease_count(&f), 1);
    assert_eq!(box_state(&f), BoxState::ReleaseFailed);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "box_release_step_failed"));
}

#[test]
fn a_failed_release_step_of_a_box_that_was_the_whole_machine_still_closes() {
    let f = leased(1);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted);
    assert_eq!(lease_count(&f), 0);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "box_release_step_failed"));
}

#[test]
fn a_failed_release_step_keeps_the_lease_rows_and_the_task_open_on_a_static_host() {
    let driver = quiet(3)
        .static_host()
        .exec_results([0, 0, 0, 1].map(|code| Ok(ExecOutcome::Exited { code })));
    let f = fx_with(driver, FakeSurface::new("/s"), 0);
    let store = f.cx.store().unwrap();
    store.plan_lease(f.id, LeaseKind::BoxRoot, "/root").unwrap();
    store.plan_lease(f.id, LeaseKind::Compose, "proj").unwrap();
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Failed);
    assert!(e.error.to_string().contains("the task stays open"));
    assert_eq!(lease_count(&f), 2);
    assert_eq!(box_state(&f), BoxState::ReleaseFailed);
}

#[test]
fn force_close_runs_the_same_release() {
    let f = fx_with(quiet(0), FakeSurface::new("/s"), 3);
    f.cx.store()
        .unwrap()
        .plan_lease(f.id, LeaseKind::Compose, "proj")
        .unwrap();
    let v = run(&f.cx, &req(true)).unwrap();
    assert!(v.box_deleted);
    assert_eq!(lease_count(&f), 0);
    assert!(f
        .driver
        .execs()
        .iter()
        .any(|(_, spec, _)| format!("{spec:?}").contains("compose")));
}

const MAC_WORKDIR: &str = "/Users/Shared/moor-newman/envchk/work";
const MAC_ROOT: &str = "/Users/Shared/moor-newman/envchk";

/// A Ready task `envchk` whose Box workdir is `<work_root>/envchk/work`, with a root lease.
fn mac_fx(driver: FakeDriver) -> Fx {
    mac_task(driver, "envchk", MAC_WORKDIR, MAC_ROOT)
}

/// A Ready task `name` whose Box workdir is `workdir`, with a root lease on `root`.
fn mac_task(driver: FakeDriver, task: &str, workdir: &str, root: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let surface = Arc::new(FakeSurface::new("/s"));
    let proc = proc(0);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let store = cx.store().unwrap();
    let name = TaskName::parse(task).unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set(dir.path().display().to_string()),
        box_workdir: Field::Set(workdir.to_owned()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            task: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    store.plan_lease(id, LeaseKind::BoxRoot, root).unwrap();
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

fn force_envchk() -> CloseReq {
    CloseReq {
        task: TaskName::parse("envchk").unwrap(),
        ..req(true)
    }
}

/// The last argument of each exec: the script the Box ran.
fn scripts(f: &Fx) -> Vec<String> {
    f.driver
        .execs()
        .iter()
        .map(|(_, spec, _)| spec.argv.last().cloned().unwrap())
        .collect()
}

/// A Mac Task's names, and how many scripts ran before the release.
struct Mac {
    task: &'static str,
    workdir: &'static str,
    root: &'static str,
    lead: usize,
}

impl Mac {
    fn new(task: &'static str, workdir: &'static str, root: &'static str, lead: usize) -> Self {
        Self {
            task,
            workdir,
            root,
            lead,
        }
    }
}

/// The Mac release ran in order against the Task root, and the Box is released and deleted.
fn assert_mac_released(f: &Fx, rows: &[crate::store::lease::LeaseRow], m: &Mac) {
    assert_eq!(box_state(f), BoxState::Deleted);
    assert_eq!(lease_count(f), 0);
    assert!(released(f));
    let key = TaskKey::new(&TaskName::parse(m.task).unwrap());
    let layout = MacLayout::find(&BoxPath::restore(m.workdir).unwrap(), &key).unwrap();
    let target = MacTarget {
        key: &key,
        layout: &layout,
    };
    let want: Vec<String> = plan(Some(&target), rows)
        .into_iter()
        .filter_map(|s| script(s, Some(&target), rows))
        .collect();
    let all = scripts(f);
    let got = all.get(m.lead..).unwrap();
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert!(g.contains(w.as_str()), "{g} lacks {w}");
    }
    let at = |needle: &str| got.iter().position(|s| s.contains(needle));
    let (stop, rm, verify) = (at("kill -TERM"), at("rm -rf \"$root\""), at("left: root"));
    assert!(stop.is_some() && stop < rm && rm < verify, "{got:?}");
    assert_eq!(verify, Some(got.len() - 1));
    let root = format!("root='{}'", m.root);
    assert!(got.get(rm.unwrap()).unwrap().contains(&root));
}

#[test]
fn force_close_of_a_mac_box_runs_the_mac_release_steps_in_order() {
    let f = mac_fx(quiet(0));
    let rows = f.cx.store().unwrap().leases(f.id).unwrap();
    let v = run(&f.cx, &force_envchk()).unwrap();
    assert!(v.box_deleted);
    assert_mac_released(&f, &rows, &Mac::new("envchk", MAC_WORKDIR, MAC_ROOT, 1));
}

#[test]
fn force_close_of_a_mac_box_with_leftovers_closes_and_warns_what_needs_manual_cleanup() {
    let left = format!("left: root {MAC_ROOT}\nleft: process 123");
    let driver = printing(0, &[(5, &left)]).exec_results((0..7).map(|i| {
        Ok(ExecOutcome::Exited {
            code: i32::from(i == 6),
        })
    }));
    let f = mac_fx(driver);
    let v = run(&f.cx, &force_envchk()).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(!v.box_deleted);
    assert_eq!(lease_count(&f), 1);
    let warnings = f.cx.take_warnings();
    let w = warnings
        .iter()
        .find(|w| w.code == "box_release_failed")
        .unwrap();
    assert!(w.message.contains(&format!("root {MAC_ROOT}")), "{w:?}");
    assert!(w.message.contains("process 123"), "{w:?}");
    assert!(w.message.contains("manual"), "{w:?}");
}

const FIELD_WORKDIR: &str = "/Users/Shared/moor-newman/envchk2/work";
const FIELD_ROOT: &str = "/Users/Shared/moor-newman/envchk2";

/// A Mac Task `envchk2` whose Box probes clean, on a branch whose remote cannot be fetched
/// when `remote_missing`.
fn field_fx(remote_missing: bool) -> Fx {
    let driver = quiet(3).peek_text(
        "box-1",
        &BoxPath::under(FIELD_WORKDIR, ".moor/boxwork").unwrap(),
        "ahead=0 dirty=0",
    );
    let f = mac_task(driver, "envchk2", FIELD_WORKDIR, FIELD_ROOT);
    let fields = Fields {
        branch: Field::Set("moor/envchk2".to_owned()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(fields))
        .unwrap();
    if remote_missing {
        f.proc.on(
            Tool::Git,
            &["ls-remote"],
            Reply::Ok(format!("{HEAD}\trefs/heads/moor/envchk2\n")),
        );
        f.proc.on(
            Tool::Git,
            &["symbolic-ref"],
            Reply::Ok("moor/envchk2\n".to_owned()),
        );
        f.proc.on(
            Tool::Git,
            &["fetch"],
            Reply::Fail {
                code: 128,
                stdout: String::new(),
                stderr: "'origin' does not appear to be a git repository".to_owned(),
            },
        );
    }
    f
}

fn close_envchk2(f: &Fx) -> moor_core::view::CloseView {
    let req = CloseReq {
        task: TaskName::parse("envchk2").unwrap(),
        ..req(false)
    };
    run(&f.cx, &req).unwrap()
}

/// A Mac Task `envchk2` whose Box probes clean; the probes exit 0 and the fourth script exits `code`.
fn mac_leased(code: i32) -> Fx {
    let driver = quiet(3)
        .peek_text(
            "box-1",
            &BoxPath::under(FIELD_WORKDIR, ".moor/boxwork").unwrap(),
            "ahead=0 dirty=0",
        )
        .exec_results([0, 0, 0, code].map(|code| Ok(ExecOutcome::Exited { code })));
    mac_task(driver, "envchk2", FIELD_WORKDIR, FIELD_ROOT)
}

/// A plain close of `envchk2`, whatever the outcome.
fn try_close_envchk2(f: &Fx) -> Result<moor_core::view::CloseView, crate::error::CommandError> {
    let req = CloseReq {
        task: TaskName::parse("envchk2").unwrap(),
        ..req(false)
    };
    run(&f.cx, &req)
}

#[test]
fn a_plain_close_of_a_mac_box_runs_the_mac_release_then_deletes_the_box() {
    let f = field_fx(false);
    let rows = f.cx.store().unwrap().leases(f.id).unwrap();
    let v = close_envchk2(&f);
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted);
    assert_mac_released(
        &f,
        &rows,
        &Mac::new("envchk2", FIELD_WORKDIR, FIELD_ROOT, 3),
    );
}

#[test]
fn a_plain_close_with_the_remote_missing_warns_and_still_runs_the_mac_release() {
    let f = field_fx(true);
    let rows = f.cx.store().unwrap().leases(f.id).unwrap();
    let v = close_envchk2(&f);
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted);
    let codes: Vec<_> = f.cx.take_warnings().into_iter().map(|w| w.code).collect();
    assert!(codes.contains(&"branch_not_synced".to_owned()), "{codes:?}");
    assert_mac_released(
        &f,
        &rows,
        &Mac::new("envchk2", FIELD_WORKDIR, FIELD_ROOT, 3),
    );
}

#[test]
fn a_retried_plain_close_after_a_failed_release_skips_the_probe_and_closes() {
    let f = mac_leased(1);
    let e = try_close_envchk2(&f).unwrap_err();
    assert!(e.error.to_string().contains("the task stays open"), "{e:?}");
    assert_eq!(box_state(&f), BoxState::ReleaseFailed);
    assert_eq!(lease_count(&f), 1);
    let first = scripts(&f).len();
    // The fake driver forgets a released Box; a real one can leave it running.
    f.driver.revive(&BoxHandle::new("box-1"));

    let v = try_close_envchk2(&f).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(v.box_deleted);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert_eq!(lease_count(&f), 0);
    let second = scripts(&f).split_off(first);
    assert!(!second.is_empty(), "the release ran again");
    assert!(second.iter().all(|s| !s.contains("boxwork")), "{second:?}");
}

#[test]
fn a_plain_close_with_a_failing_release_keeps_the_task_open_every_time() {
    let driver = quiet(3)
        .peek_text(
            "box-1",
            &BoxPath::under(FIELD_WORKDIR, ".moor/boxwork").unwrap(),
            "ahead=0 dirty=0",
        )
        .exec_results([0, 0, 0].map(|code| Ok(ExecOutcome::Exited { code })));
    let driver = driver.exec_results((0..60).map(|_| Ok(ExecOutcome::Exited { code: 1 })));
    let f = mac_task(driver, "envchk2", FIELD_WORKDIR, FIELD_ROOT);
    for n in 1..=3 {
        let e = try_close_envchk2(&f).unwrap_err();
        assert!(e.error.to_string().contains("the task stays open"), "{e:?}");
        assert_eq!(box_state(&f), BoxState::ReleaseFailed);
        assert_eq!(lease_count(&f), 1);
        let calls = f.driver.calls();
        let releases = calls.iter().filter(|c| c.starts_with("release")).count();
        assert_eq!(releases, n, "{calls:?}");
        f.driver.revive(&BoxHandle::new("box-1"));
    }
    let task = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    assert_ne!(task.state, TaskState::Closed);
}

fn driver_error_fx(rows: bool) -> Fx {
    let driver = quiet(3).fail_release_always(DriverError::Failed {
        summary: "boom".into(),
    });
    let f = fx_with(driver, FakeSurface::new("/s"), 0);
    if rows {
        let store = f.cx.store().unwrap();
        store.plan_lease(f.id, LeaseKind::Compose, "proj").unwrap();
    }
    f
}

#[test]
fn a_plain_close_with_a_driver_error_and_no_lease_rows_closes_with_a_warning() {
    let f = driver_error_fx(false);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.state, TaskState::Closed);
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "box_release_failed"));
}

#[test]
fn a_plain_close_with_a_driver_error_and_lease_rows_stays_open_and_points_at_force() {
    let f = driver_error_fx(true);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert!(e.error.to_string().contains("the task stays open"), "{e:?}");
    let task = f.cx.store().unwrap().task(f.id).unwrap().unwrap();
    assert_ne!(task.state, TaskState::Closed);
    let hint = e.hints.join(" ");
    for want in ["moor close --force", "manual cleanup", "unpushed-work"] {
        assert!(hint.contains(want), "{want}: {hint}");
    }
}
