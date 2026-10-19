//! Tests for the Box work probe.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Tool;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use super::{probe, BoxWork};
use crate::cx::Cx;
use crate::store::{BoxRow, TaskRow};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
/// The handle the fake driver gives its first Box.
const FIRST_HANDLE: &str = "box-1";
const WORKDIR: &str = "/work/cbx_1/repo";

struct Rig {
    cx: Cx,
    driver: Arc<FakeDriver>,
    proc: Arc<ScriptedProc>,
    task: TaskRow,
    boxed: BoxRow,
}

fn result_path() -> BoxPath {
    BoxPath::under(WORKDIR, ".moor/boxwork").unwrap()
}

fn rig(dir: &Path, driver: FakeDriver) -> Rig {
    let driver = Arc::new(driver);
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        dir,
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        Arc::clone(&proc) as Arc<_>,
    );
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: BoxId::new(1),
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let (task, boxed) = rows(dir, got.handle);
    let r = Rig {
        cx,
        driver,
        proc,
        task,
        boxed,
    };
    r.proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok(format!("{HEAD}\n")),
    );
    r
}

fn rows(dir: &Path, handle: BoxHandle) -> (TaskRow, BoxRow) {
    let boxed = BoxRow {
        provider: ProviderId::new("p"),
        handle: Some(handle),
        state: BoxState::Ready,
        attempted_handle: None,
        workdir: Some(WORKDIR.to_owned()),
        egress: None,
        sleep_reason: None,
        updated_at: Ts::from_millis(0),
    };
    let task = TaskRow {
        id: BoxId::new(1),
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: dir.display().to_string(),
        issue: None,
        goal: None,
        base: None,
        branch: None,
        worktree: Some(dir.display().to_string()),
        bound: false,
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
    (task, boxed)
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
    probe(&r.cx, &r.task, &r.boxed)
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
    assert_eq!(spec.sync, SyncMode::None);
    assert_eq!(spec.cwd, None);
    let mut argv = spec.argv.iter().map(String::as_str);
    assert_eq!((argv.next(), argv.next()), (Some("sh"), Some("-c")));
    assert!(argv.next().is_some_and(|script| script.contains(HEAD)));
    let peeks = r.driver.peeks();
    assert_eq!(peeks.len(), 1);
    assert!(peeks
        .iter()
        .all(|(_, path, _)| path.as_str().ends_with(".moor/boxwork")));
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
fn missing_worktree_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let mut r = with_result(d.path(), "ahead=0 dirty=0");
    r.task.worktree = None;
    assert!(is_unknown(&run(&r)));
    assert!(r.driver.execs().is_empty());
}

fn with_head_reply(dir: &Path, reply: Reply) -> (Rig, Cx) {
    let r = rig(dir, FakeDriver::new());
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Git, &["rev-parse", "HEAD"], reply);
    let cx = Cx::fake(
        dir,
        dir,
        Arc::clone(&r.driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir)),
        proc as Arc<_>,
    );
    (r, cx)
}

#[test]
fn host_git_failure_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let reply = Reply::Fail {
        code: 128,
        stdout: String::new(),
        stderr: "fatal: not a git repository".to_owned(),
    };
    let (r, cx) = with_head_reply(d.path(), reply);
    assert!(is_unknown(&probe(&cx, &r.task, &r.boxed)));
    assert!(r.driver.execs().is_empty());
}

#[test]
fn host_head_that_is_not_a_full_sha_is_unknown() {
    let d = tempfile::tempdir().unwrap();
    let (r, cx) = with_head_reply(d.path(), Reply::Ok("HEAD\n".to_owned()));
    assert!(is_unknown(&probe(&cx, &r.task, &r.boxed)));
    assert!(r.driver.execs().is_empty());
}

#[test]
fn branch_name_base_is_skipped() {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), FakeDriver::new());
    r.task.base = Some("origin/main".to_owned());
    let _ = run(&r);
    let script = first_exec(&r).argv.concat();
    assert!(script.contains(HEAD));
    assert!(!script.contains("origin/main"));
}

#[test]
fn sha_base_is_saved_too() {
    let d = tempfile::tempdir().unwrap();
    let mut r = rig(d.path(), FakeDriver::new());
    r.task.base = Some(OTHER.to_owned());
    let _ = run(&r);
    let script = first_exec(&r).argv.concat();
    assert!(script.contains(HEAD));
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
    let plan = super::plan(&r.cx, &r.task, &r.boxed).unwrap();
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
