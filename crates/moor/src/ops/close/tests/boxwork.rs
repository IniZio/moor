//! Close checks the Box for work that exists nowhere else.

use moor_core::boxscript::boxwork::{BOXWORK_FILE, BOXWORK_NO_CHECKOUT};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::{BoxName, CloseReq, ErrorKind, ProviderId};

use std::sync::Arc;

use moor_core::BoxPath;

use super::{admit, box_state, fx_box, proc, req, row, run, Fx};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;
use moor_core::seam::proc::Tool;

fn surface() -> FakeSurface {
    FakeSurface::new("/s")
}

/// A Ready Box whose probe answers `text`; `None` leaves the answer unreadable.
fn fx_probe(text: Option<&str>) -> Fx {
    fx_box(FakeDriver::new(), surface(), 0, text)
}

/// A Ready Box in a worktree named `repo`, so a woken Box (which the fake driver
/// places at `/work/cbx_1/repo`) reads `text` from its probe result.
pub(super) fn fx_woken(text: &str) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("repo");
    std::fs::create_dir(&worktree).unwrap();
    let path = BoxPath::under("/work/cbx_1/repo", ".moor/boxwork").unwrap();
    let driver = Arc::new(FakeDriver::new().peek_text("box-1", &path, text));
    let surface = Arc::new(surface());
    let proc = proc(0);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        surface.clone(),
        proc.clone(),
    );
    let id = admit(&cx, &driver, &worktree);
    cx.store()
        .unwrap()
        .apply(Decision::new(id).set(Fields {
            box_workdir: Field::Set("/work/cbx_1/repo".to_owned()),
            ..Fields::default()
        }))
        .unwrap();
    Fx {
        dir,
        cx,
        driver,
        surface,
        proc,
        id,
    }
}

/// Puts the Box and its machine to sleep.
pub(super) fn put_to_sleep(f: &Fx) {
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
    assert_eq!(box_state(f), BoxState::Asleep);
}

fn count(f: &Fx, prefix: &str) -> usize {
    f.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with(prefix))
        .count()
}

/// Execs that ran the unsaved-work probe, as opposed to other closing steps.
fn probes(f: &Fx) -> usize {
    f.driver
        .execs()
        .iter()
        .filter(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(BOXWORK_FILE)))
        .count()
}

/// The error message with its hints, as a user reads them.
fn text(e: &CommandError) -> String {
    format!("{}\n{}", e.error, e.hints.join("\n"))
}

/// Closing without `--force` is refused and changes nothing.
fn refused(f: &Fx) -> CommandError {
    let (lifecycle_before, boxed) = (row(f).lifecycle, box_state(f));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert_eq!(row(f).lifecycle, lifecycle_before);
    assert_eq!(box_state(f), boxed);
    assert_eq!(count(f, "release"), 0);
    e
}

#[test]
fn close_refuses_a_box_with_an_unpushed_commit_when_the_host_worktree_is_clean() {
    let f = fx_probe(Some("ahead=1 dirty=0"));
    let t = text(&refused(&f));
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("not on the host"), "{t}");
    assert!(t.contains("1 unpushed commit"), "{t}");
    for cmd in [
        "moor run t -- git bundle create .moor/rescue.bundle HEAD --not --remotes",
        "moor cp t .moor/rescue.bundle .",
        "git fetch ./rescue.bundle HEAD:",
        "moor close t --force",
    ] {
        assert!(t.contains(cmd), "missing {cmd:?} in {t}");
    }
    assert_eq!(count(&f, "exec"), 1);
}

#[test]
fn close_refuses_a_box_with_only_uncommitted_changes() {
    let f = fx_probe(Some("ahead=0 dirty=3"));
    let t = text(&refused(&f));
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("3 uncommitted file"), "{t}");
    assert!(t.contains("git push moor HEAD:refs/heads/"), "{t}");
    assert!(!t.contains("bundle"), "{t}");
}

#[test]
fn close_proceeds_when_the_box_probe_is_clean() {
    let f = fx_probe(Some("ahead=0 dirty=0"));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(box_state(&f), BoxState::Deleted);
    assert_eq!(probes(&f), 2, "the probe ran before and after the stop");
}

#[test]
fn close_refuses_when_the_box_cannot_be_checked() {
    let f = fx_probe(None);
    let t = text(&refused(&f));
    assert!(
        t.contains("could not check the Box for unsaved work"),
        "{t}"
    );
}

#[test]
fn close_wakes_a_sleeping_box_and_refuses_when_it_holds_unsaved_work() {
    let f = fx_woken("ahead=2 dirty=0");
    put_to_sleep(&f);
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(text(&e).contains("the Box holds work"), "{}", text(&e));
    assert_eq!(count(&f, "wake"), 1);
    assert_eq!(count(&f, "exec"), 1);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_ne!(box_state(&f), BoxState::Deleted);
    assert_eq!(count(&f, "release"), 0);
}

#[test]
fn close_wakes_a_sleeping_box_and_proceeds_when_it_is_clean() {
    let f = fx_woken("ahead=0 dirty=0");
    put_to_sleep(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(count(&f, "wake"), 1);
    assert_eq!(probes(&f), 2);
}

#[test]
fn close_refuses_and_leaves_the_box_asleep_when_it_cannot_be_woken() {
    let driver = FakeDriver::new().fail_wake(DriverError::Failed {
        summary: "boom".into(),
    });
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    put_to_sleep(&f);
    let t = text(&refused(&f));
    assert!(t.contains("could not wake the Box"), "{t}");
    assert_eq!(box_state(&f), BoxState::Asleep);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_proceeds_when_the_driver_no_longer_has_the_box_on_probe() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    let h = row(&f).boxed.unwrap().handle.unwrap();
    f.driver.forget(&h);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
}

#[test]
fn close_of_a_box_the_driver_no_longer_lists_never_reads_the_host_head() {
    let mut f = fx_probe(Some("ahead=1 dirty=1"));
    let headless = ScriptedProc::new();
    headless.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".to_owned()));
    headless.on(Tool::Git, &["status"], Reply::Ok(String::new()));
    let headless = Arc::new(headless);
    f.cx = Cx::fake(
        f.dir.path(),
        f.dir.path(),
        f.driver.clone(),
        f.surface.clone(),
        headless.clone(),
    );
    let h = row(&f).boxed.unwrap().handle.unwrap();
    f.driver.forget(&h);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    let head = headless
        .calls()
        .into_iter()
        .filter(|c| c.args == ["rev-parse", "HEAD"])
        .count();
    assert_eq!(head, 0);
}

#[test]
fn close_proceeds_when_the_driver_no_longer_has_the_box_on_wake() {
    let driver = FakeDriver::new().fail_wake(DriverError::NotFound);
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    put_to_sleep(&f);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
}

#[test]
fn force_closes_a_box_with_unsaved_work_without_probing() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(probes(&f), 0);
    assert_eq!(count(&f, "wake"), 0);
}

#[test]
fn force_closes_a_sleeping_box_without_waking_or_probing_it() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    put_to_sleep(&f);
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(count(&f, "exec"), 0);
    assert_eq!(count(&f, "wake"), 0);
}

#[test]
fn close_skips_the_probe_for_a_box_the_driver_no_longer_has() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    assert_eq!(box_state(&f), BoxState::Gone);
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_skips_the_probe_for_a_box_that_never_got_a_handle() {
    let f = fx_probe(Some("ahead=1 dirty=1"));
    let store = f.cx.store().unwrap();
    let name = BoxName::parse_allow_bare("u").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
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
    let cause = "no capacity".to_owned();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::SetupFailed {
                    cause: cause.clone(),
                    which: None,
                })
                .box_input(BoxInput::AcquireFailed {
                    cause,
                    attempted: None,
                }),
        )
        .unwrap();
    let v = run(
        &f.cx,
        &CloseReq {
            target: name.into(),
            force: false,
            trust_moor_yaml: false,
        },
    )
    .unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(count(&f, "exec"), 0);
}

#[test]
fn close_refuses_when_the_agent_writes_between_the_probe_and_the_stop() {
    let f = fx_probe(Some("ahead=0 dirty=0"));
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-gone"))
        .unwrap();
    let driver = f.driver.clone();
    let runs = std::sync::atomic::AtomicU32::new(0);
    f.driver.on_exec(move || {
        if runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let path = BoxPath::under("/work/box-1", ".moor/boxwork").unwrap();
            driver.set_peek_text("box-1", &path, "ahead=1 dirty=2");
        }
    });
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let t = text(&e);
    assert!(t.contains("the Box holds work"), "{t}");
    assert!(t.contains("the agent was stopped"), "{t}");
    assert!(t.contains("moor start t --resume"), "{t}");
    // The recheck shares a batch with the report index script, so that script runs too.
    assert_eq!(count(&f, "exec"), 3);
    assert_eq!(count(&f, "release"), 0);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_ne!(box_state(&f), BoxState::Deleted);
    assert!(f.cx.store().unwrap().live_session(f.id).unwrap().is_none());
}

#[test]
fn force_does_not_probe_before_or_after_stopping_the_agent() {
    let f = fx_probe(Some("ahead=4 dirty=1"));
    f.cx.store()
        .unwrap()
        .begin_session(f.id, "s-1", Some("p-gone"))
        .unwrap();
    let v = run(&f.cx, &req(true)).unwrap();
    assert!(v.agent_stopped);
    assert_eq!(probes(&f), 0);
}

const NO_CHECKOUT: ExecOutcome = ExecOutcome::Exited {
    code: BOXWORK_NO_CHECKOUT,
};

#[test]
fn close_refuses_a_box_with_no_git_checkout_like_an_unreadable_probe() {
    let driver = FakeDriver::new().exec_results([Ok(NO_CHECKOUT)]);
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    let t = text(&refused(&f));
    assert!(
        t.contains("could not check the Box for unsaved work"),
        "{t}"
    );
}

#[test]
fn close_refuses_when_the_recheck_finds_no_git_checkout() {
    let ok = Ok(ExecOutcome::Exited { code: 0 });
    let driver = FakeDriver::new().exec_results([ok, Ok(NO_CHECKOUT)]);
    let f = fx_box(driver, surface(), 0, Some("ahead=0 dirty=0"));
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let t = text(&e);
    assert!(
        t.contains("could not check the Box for unsaved work"),
        "{t}"
    );
    assert_eq!(count(&f, "release"), 0);
    assert_ne!(row(&f).lifecycle, BoxLifecycle::Closed);
}
