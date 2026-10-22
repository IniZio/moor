//! Tests for the Box work probe.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Tool;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{probe, BoxWork};
use crate::cx::Cx;
use crate::store::{BoxRow, DriverRow};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
/// The handle the fake driver gives its first Box.
const FIRST_HANDLE: &str = "box-1";
const WORKDIR: &str = "/work/cbx_1/repo";

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    proc: Arc<ScriptedProc>,
    row: BoxRow,
    boxed: DriverRow,
}

fn result_path() -> BoxPath {
    BoxPath::under(WORKDIR, ".moor/boxwork").unwrap()
}

fn rig(dir: &Path, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new()),
        Arc::clone(&proc) as Arc<_>,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: BoxId::new(1),
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
        })
        .unwrap();
    let (row, boxed) = rows(got.handle);
    Rig {
        cx,
        driver,
        proc,
        row,
        boxed,
    }
}

fn rows(handle: BoxHandle) -> (BoxRow, DriverRow) {
    let boxed = DriverRow {
        provider: ProviderId::new("p"),
        handle: Some(handle),
        state: BoxState::Ready,
        attempted_handle: None,
        workdir: Some(WORKDIR.to_owned()),
        egress: None,
        sleep_reason: None,
        updated_at: Ts::from_millis(0),
    };
    let row = BoxRow {
        id: BoxId::new(1),
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
        user: None,
        issue: None,
        goal: None,
        base: None,
        branch: None,
        workspace_id: None,
        agent_session_id: None,
        lifecycle: BoxLifecycle::Ready,
        session: SessionState::Working,
        state_reason: None,
        intent: None,
        intent_at: None,
        setup_failed_at: None,
        idle_since: None,
        launch_failure: None,
        created_at: Ts::from_millis(0),
        updated_at: Ts::from_millis(0),
        boxed: Some(boxed.clone()),
    };
    (row, boxed)
}

fn with_result(dir: &Path, text: &str) -> Rig {
    rig(
        dir,
        FakeDriver::new().peek_text(FIRST_HANDLE, &result_path(), text),
    )
}

fn first_exec(r: &Rig) -> ExecSpec {
    r.driver.execs().into_iter().next().unwrap().1
}

fn run(r: &Rig) -> BoxWork {
    probe(&r.cx, &r.row, &r.boxed)
}

fn is_unknown(w: &BoxWork) -> bool {
    matches!(w, BoxWork::Unknown(_))
}

#[test]
fn clean_box_is_clean_and_the_probe_is_shaped_right() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=0 dirty=0");
    assert_eq!(run(&r), BoxWork::Clean);
    assert_eq!(r.driver.execs().len(), 1);
    let spec = first_exec(&r);
    assert_eq!(spec.cwd, None);
    let mut argv = spec.argv.iter().map(String::as_str);
    assert_eq!((argv.next(), argv.next()), (Some("sh"), Some("-c")));
    assert!(argv
        .next()
        .is_some_and(|script| script.contains(".moor/boxwork")));
    let peeks = r.driver.peeks();
    assert_eq!(peeks.len(), 1);
    assert!(peeks
        .iter()
        .all(|(_, path)| path.as_str().ends_with(".moor/boxwork")));
}

#[test]
fn counts_become_unsaved() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=2 dirty=1");
    assert_eq!(run(&r), BoxWork::Unsaved { ahead: 2, dirty: 1 });
}

#[test]
fn only_dirty_files_are_unsaved() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=0 dirty=3");
    assert_eq!(run(&r), BoxWork::Unsaved { ahead: 0, dirty: 3 });
}

#[test]
fn exec_errors_are_unknown() {
    for e in [
        DriverError::Unavailable("down".to_owned()),
        DriverError::Timeout(Duration::from_secs(60)),
    ] {
        let d = tempfile::tempdir().unwrap();
        let r = rig(d.path(), FakeDriver::new().exec_results([Err(e)]));
        assert!(is_unknown(&run(&r)));
        assert!(r.driver.peeks().is_empty());
    }
}

#[test]
fn interrupted_exec_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Interrupted)]);
    assert!(is_unknown(&run(&rig(d.path(), driver))));
}

#[test]
fn non_zero_exit_is_unknown_and_names_the_code() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 7 })]);
    let r = rig(d.path(), driver);
    assert!(matches!(run(&r), BoxWork::Unknown(why) if why.contains('7')));
    assert!(r.driver.peeks().is_empty());
}

#[test]
fn missing_result_file_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    assert!(is_unknown(&run(&rig(d.path(), FakeDriver::new()))));
}

#[test]
fn failing_peek_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .peek_text(FIRST_HANDLE, &result_path(), "ahead=0 dirty=0")
        .fail_peek(DriverError::Unavailable("x".to_owned()));
    assert!(is_unknown(&run(&rig(d.path(), driver))));
}

#[test]
fn garbage_result_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    for text in ["", "nonsense", "ahead=x dirty=1"] {
        assert!(is_unknown(&run(&with_result(d.path(), text))));
    }
}

#[test]
fn missing_handle_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let mut r = with_result(d.path(), "ahead=0 dirty=0");
    r.boxed.handle = None;
    assert!(is_unknown(&run(&r)));
    assert!(r.driver.execs().is_empty());
}

#[test]
fn the_probe_reads_nothing_from_the_host_checkout() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=0 dirty=0");
    assert_eq!(run(&r), BoxWork::Clean);
    let git: Vec<_> = r
        .proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Git)
        .collect();
    assert!(git.is_empty(), "{git:?}");
}

#[test]
fn branch_name_base_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), FakeDriver::new());
    r.row.base = Some("origin/main".to_owned());
    let _ = run(&r);
    let script = first_exec(&r).argv.concat();
    assert!(!script.contains("origin/main"));
}

#[test]
fn sha_base_is_saved_too() {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), FakeDriver::new());
    r.row.base = Some(OTHER.to_owned());
    let _ = run(&r);
    let script = first_exec(&r).argv.concat();
    assert!(script.contains(OTHER));
}

#[test]
fn unrecorded_workdir_is_asked_of_the_driver() {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), FakeDriver::new());
    r.boxed.workdir = None;
    let _ = run(&r);
    assert!(r.driver.calls().iter().any(|c| c.starts_with("workdir")));
    assert_eq!(r.driver.execs().len(), 1);
}

#[test]
fn probe_is_one_box_call() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=0 dirty=0");
    let before = r.driver.box_calls().len();
    assert_eq!(run(&r), BoxWork::Clean);
    let calls = r.driver.box_calls();
    assert_eq!(calls.get(before..), Some(&["batch".to_owned()][..]));
}

#[test]
fn probe_script_enters_the_workdir_first() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=0 dirty=0");
    assert_eq!(run(&r), BoxWork::Clean);
    let script = first_exec(&r).argv.concat();
    assert!(script.contains(&format!("cd '{WORKDIR}' || exit 1\n")));
}

#[test]
fn plan_then_read_matches_probe() {
    let d = tempfile::tempdir().unwrap();
    let r = with_result(d.path(), "ahead=2 dirty=0");
    let plan = super::plan(&r.cx, &r.row, &r.boxed).unwrap();
    assert_eq!(plan.steps.len(), 2);
    let mut none = std::iter::empty();
    assert_eq!(
        super::read(&plan, &mut none),
        BoxWork::from("the probe left no result")
    );
}

#[test]
fn a_workdir_with_no_git_checkout_is_no_checkout() {
    let d = tempfile::tempdir().unwrap();
    let code = moor_core::boxscript::boxwork::BOXWORK_NO_CHECKOUT;
    let driver = FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code })]);
    assert_eq!(run(&rig(d.path(), driver)), BoxWork::NoCheckout);
}
