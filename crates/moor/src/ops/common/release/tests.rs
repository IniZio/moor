//! Tests for the Box release.

use std::sync::Arc;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome, Released};
use moor_core::seam::proc::Tool;
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName, Warning};

use super::steps::{plan, script, MacTarget};
use super::testkit::{framed, BANNER};
use super::{release_box, ReleaseReport};
use crate::cx::Cx;
use crate::store::lease::LeaseKind;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const MAC_WORKDIR: &str = "/Users/box/moor/demo/work/repo";

struct Rig {
    _dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
    handle: BoxHandle,
}

/// A git that answers the teardown lookups.
fn git() -> Arc<ScriptedProc> {
    let p = ScriptedProc::new();
    p.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok("abc\n".to_owned()),
    );
    p.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    p.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/main/.git\n".to_owned()),
    );
    Arc::new(p)
}

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// A Task named `demo` with a Ready Box at `workdir` and a root and a compose lease.
fn rig(driver: FakeDriver, workdir: &str) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir.path())),
        git(),
    );
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
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
            name: TaskName::parse("demo").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        });
    store.apply(d).unwrap();
    store.plan_lease(id, LeaseKind::BoxRoot, "/root").unwrap();
    store.plan_lease(id, LeaseKind::Compose, "proj").unwrap();
    Rig {
        _dir: dir,
        cx,
        driver,
        id,
        handle: got.handle,
    }
}

fn mac(driver: FakeDriver) -> Rig {
    rig(driver, MAC_WORKDIR)
}

fn go(r: &Rig) -> ReleaseReport {
    let store = r.cx.store().unwrap();
    let row = store.task(r.id).unwrap().unwrap();
    release_box(&r.cx, store, &row, &r.handle, false).unwrap()
}

/// Stdout for the seven Mac scripts, `at` replaced by `text`.
fn stdout_at(at: u32, text: &str) -> Vec<String> {
    (0..7)
        .map(|i| framed(i, if i == at { text } else { "" }))
        .collect()
}

/// A driver whose scripts print the given stdout in order.
fn printing(outs: &[String]) -> FakeDriver {
    FakeDriver::new().run_stdouts(outs.iter().map(String::as_str))
}

/// A driver whose seven Mac scripts print nothing of their own.
fn quiet() -> FakeDriver {
    printing(&stdout_at(99, ""))
}

fn warnings(r: &Rig) -> Vec<Warning> {
    r.cx.take_warnings()
}

fn codes(ws: &[Warning]) -> Vec<&str> {
    ws.iter().map(|w| w.code.as_str()).collect()
}

#[test]
fn a_mac_box_runs_every_step_in_the_planned_order() {
    let r = mac(quiet());
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    go(&r);
    let key = TaskKey::new(&TaskName::parse("demo").unwrap());
    let layout = MacLayout::find(&BoxPath::restore(MAC_WORKDIR).unwrap(), &key).unwrap();
    let target = MacTarget {
        key: &key,
        layout: &layout,
    };
    let want: Vec<String> = plan(Some(&target), &rows)
        .into_iter()
        .filter_map(|s| script(s, Some(&target), &rows))
        .collect();
    let got = scripts(&r);
    assert_eq!(got.len(), 7, "{got:?}");
    for (g, w) in got.iter().zip(&want) {
        assert!(g.contains(w.as_str()), "{g} lacks {w}");
    }
    assert!(got.first().unwrap().contains("TERM"));
    assert!(got.get(4).unwrap().contains("proj"));
}

#[test]
fn every_release_script_leaves_the_root_first_runs_batched_and_no_login_home_is_read() {
    let r = mac(quiet());
    go(&r);
    let got = scripts(&r);
    assert_eq!(got.len(), 7, "{got:?}");
    assert!(got
        .iter()
        .all(|s| s.starts_with("exec 2>&1\ncd / || exit 1\n")));
    assert!(!got.iter().any(|s| s.contains("printf %s \"$HOME\"")));
    assert!(!r.driver.box_calls().iter().any(|c| c == "exec"));
}

#[test]
fn the_release_shell_has_no_env() {
    let r = mac(quiet());
    go(&r);
    let execs = r.driver.execs();
    assert!(!execs.is_empty());
    assert!(execs.iter().all(|(_, spec, _)| spec.env.is_empty()));
}

#[test]
fn a_clean_release_drops_every_lease_row_and_releases_the_driver() {
    let r = mac(quiet());
    let report = go(&r);
    assert!(report.released);
    assert_eq!(report.driver, Ok(Released::Deleted));
    assert!(report.left.is_empty());
    assert!(r.cx.store().unwrap().leases(r.id).unwrap().is_empty());
    assert!(r.driver.calls().contains(&"release box-1".to_owned()));
    assert!(warnings(&r).is_empty());
}

#[test]
fn a_failing_kill_step_does_not_stop_the_later_steps() {
    let driver =
        printing(&stdout_at(1, "kill went wrong")).exec_results([exit(0), exit(1)].map(Ok));
    let r = mac(driver);
    r.cx.store()
        .unwrap()
        .plan_lease(r.id, LeaseKind::Process, "tag")
        .unwrap();
    let report = go(&r);
    assert_eq!(r.driver.execs().len(), 7);
    assert!(!report.released);
    assert!(r.driver.calls().contains(&"release box-1".to_owned()));
    check_kill_rows(&r);
    let ws = warnings(&r);
    assert_eq!(codes(&ws), ["box_release_step_failed"]);
    let msg = &ws.first().unwrap().message;
    assert!(msg.starts_with("kill the process tree failed: kill went wrong"));
    assert!(!msg.contains("tries again"), "{msg}");
    assert_eq!(report.left.len(), 1);
}

fn check_kill_rows(r: &Rig) {
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    let row = |k| rows.iter().find(|r| r.kind == k).unwrap();
    let process = row(LeaseKind::Process);
    assert_eq!(process.attempts, 1);
    assert!(process
        .failure
        .as_deref()
        .unwrap()
        .contains("kill went wrong"));
    assert_eq!(row(LeaseKind::Compose).attempts, 1);
    assert_eq!(row(LeaseKind::Compose).failure, None);
}

#[test]
fn a_verify_that_finds_something_left_keeps_the_rows() {
    let driver = printing(&stdout_at(6, "left: root /Users/box/moor/demo"))
        .exec_results((0..7).map(|i| Ok(exit(i32::from(i == 6)))));
    let r = mac(driver);
    let report = go(&r);
    assert!(!report.released);
    assert_eq!(report.left, ["root /Users/box/moor/demo"]);
    assert_eq!(r.cx.store().unwrap().leases(r.id).unwrap().len(), 2);
    assert!(r.driver.calls().contains(&"release box-1".to_owned()));
}

#[test]
fn a_driver_failure_keeps_the_rows() {
    let driver = quiet().fail_release(DriverError::Failed {
        summary: "gone wrong".into(),
    });
    let r = mac(driver);
    let report = go(&r);
    assert!(report.driver.is_err());
    assert!(!report.released);
    assert_eq!(r.cx.store().unwrap().leases(r.id).unwrap().len(), 2);
}

#[test]
fn a_box_outside_the_mac_layout_runs_the_teardown_then_the_driver_release() {
    let r = rig(printing(&[framed(0, "")]), "/work/box-1");
    let report = go(&r);
    let execs = r.driver.execs();
    assert_eq!(execs.len(), 1, "{execs:?}");
    let only = &execs.first().unwrap().1;
    assert!(only.argv.last().unwrap().contains("docker compose"));
    assert!(warnings(&r).is_empty());
    assert_eq!(r.driver.calls().last().unwrap(), "release box-1");
    assert!(report.released);
}

/// A Task `demo` whose Box workdir is not recorded and that holds one extra lease row.
fn unrecorded(kind: LeaseKind, path: &str) -> Rig {
    let r = rig(quiet(), "");
    r.cx.store().unwrap().plan_lease(r.id, kind, path).unwrap();
    r
}

fn scripts(r: &Rig) -> Vec<String> {
    let execs = r.driver.execs();
    execs
        .iter()
        .map(|(_, spec, _)| spec.argv.last().cloned().unwrap())
        .collect()
}

#[test]
fn a_lease_path_under_the_key_makes_an_unrecorded_box_a_mac_box() {
    for kind in [LeaseKind::BoxDir, LeaseKind::BoxRoot, LeaseKind::Home] {
        let r = unrecorded(kind, "/Users/box/moor/demo/work/repo");
        go(&r);
        let got = scripts(&r);
        assert_eq!(got.len(), 7, "{kind:?}: {got:?}");
        let at = |needle: &str| got.iter().position(|s| s.contains(needle));
        let rm = got
            .iter()
            .position(|s| s.contains("rm ") && s.contains("/Users/box/moor/demo"))
            .unwrap();
        let verify = got.len() - 1;
        assert!(rm < verify, "{got:?}");
        assert!(at("TERM").is_some());
    }
}

#[test]
fn a_lease_path_of_a_longer_key_does_not_make_a_mac_box() {
    let r = unrecorded(LeaseKind::BoxDir, "/Users/box/moor/demo-two/work/repo");
    go(&r);
    let got = scripts(&r);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got.first().unwrap().contains("docker compose"));
}

#[test]
fn a_recorded_non_mac_workdir_ignores_a_lease_path_equal_to_it() {
    let r = rig(printing(&[framed(0, "")]), "/work/demo");
    r.cx.store()
        .unwrap()
        .plan_lease(r.id, LeaseKind::BoxDir, "/work/demo")
        .unwrap();
    go(&r);
    let got = scripts(&r);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got.first().unwrap().contains("docker compose"));
    assert!(!got.iter().any(|s| s.contains("rm -rf")), "{got:?}");
}

#[test]
fn a_recorded_non_mac_workdir_ignores_a_lease_path_under_the_key() {
    let r = rig(printing(&[framed(0, "")]), "/work/demo");
    r.cx.store()
        .unwrap()
        .plan_lease(r.id, LeaseKind::BoxDir, "/x/demo/work/repo")
        .unwrap();
    go(&r);
    let got = scripts(&r);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got.first().unwrap().contains("docker compose"));
    for needle in ["TERM", "rm ", "kill"] {
        assert!(!got.iter().any(|s| s.contains(needle)), "{needle}: {got:?}");
    }
}

#[test]
fn a_recorded_workdir_wins_over_a_lease_path() {
    let r = rig(quiet(), MAC_WORKDIR);
    r.cx.store()
        .unwrap()
        .plan_lease(r.id, LeaseKind::BoxDir, "/Other/moor/demo/work/repo")
        .unwrap();
    go(&r);
    let got = scripts(&r);
    assert!(
        got.iter().any(|s| s.contains("/Users/box/moor/demo")),
        "{got:?}"
    );
    assert!(
        !got.iter().any(|s| s.contains("/Other/moor/demo")),
        "{got:?}"
    );
}

#[test]
fn an_orphaned_ibtoold_becomes_a_warning_and_does_not_keep_the_rows() {
    let r = mac(printing(&stdout_at(6, "warn: ibtoold 77 ppid 1")));
    let report = go(&r);
    assert!(report.released);
    let ws = warnings(&r);
    assert_eq!(codes(&ws), ["orphaned_ibtoold"]);
    assert!(ws.first().unwrap().message.contains("ibtoold 77 ppid 1"));
}

#[test]
fn banner_text_around_the_output_is_never_parsed() {
    let outs = stdout_at(3, "failed AAAA-1: no such device");
    let r = mac(printing(&outs));
    let report = go(&r);
    assert!(!report.released);
    assert_eq!(report.left, ["AAAA-1: no such device"]);
    let ws = warnings(&r);
    assert!(ws
        .first()
        .unwrap()
        .message
        .contains("AAAA-1: no such device"));
    assert!(!ws.first().unwrap().message.contains("static SSH"));
}

#[test]
fn a_verify_report_between_markers_is_read_through_banner_text() {
    let r = mac(printing(&stdout_at(6, "left: root /Users/box/moor/demo")));
    let report = go(&r);
    assert_eq!(report.left, ["root /Users/box/moor/demo"]);
}

#[test]
fn output_without_markers_fails_the_step_with_a_plain_summary() {
    let mut outs = stdout_at(99, "");
    *outs.last_mut().unwrap() = format!("{BANNER}left: root /bogus\n");
    let r = mac(printing(&outs));
    let report = go(&r);
    assert!(!report.released);
    assert_eq!(report.left.len(), 1);
    assert!(report
        .left
        .first()
        .unwrap()
        .starts_with("the Box printed no release output; last output: "));
    let ws = warnings(&r);
    assert_eq!(codes(&ws), ["box_release_step_failed"]);
    assert!(ws
        .first()
        .unwrap()
        .message
        .starts_with("verify nothing is left failed: the Box printed no release output"));
}

/// A last script (verify) that prints `BEGIN`, then `tail`, and never `END`.
fn unterminated(tail: &str) -> Rig {
    let mut outs = stdout_at(99, "");
    let n = super::testkit::nonce(6);
    *outs.last_mut().unwrap() = format!("{BANNER}MOOR_RELEASE_BEGIN {n}\n{tail}\n");
    mac(printing(&outs))
}

fn first_warning(r: &Rig) -> String {
    warnings(r).remove(0).message
}

#[test]
fn a_shell_syntax_error_after_begin_is_in_the_failure_text() {
    let err = "/bin/sh: line 3: syntax error near unexpected token ';;'";
    let r = unterminated(err);
    let report = go(&r);
    assert!(report.left.first().unwrap().contains(err));
    assert!(first_warning(&r).contains(err));
}

#[test]
fn a_long_unterminated_tail_is_bounded() {
    let many: Vec<String> = (0..20).map(|i| format!("line{i:02}")).collect();
    let r = unterminated(&many.join("\n"));
    let left = go(&r).left.remove(0);
    assert!(left.contains("line19") && left.contains("line15"));
    assert!(!left.contains("line14"));
    let wide = unterminated(&"x".repeat(2000));
    let left = go(&wide).left.remove(0);
    assert!(left.matches("xx").count() <= 200);
}

#[test]
fn a_secret_in_the_unterminated_tail_is_redacted() {
    let nonce = "A".repeat(43);
    let r = unterminated(&format!("curl http://moor:{nonce}@127.0.0.1:1/ failed"));
    let left = go(&r).left.remove(0);
    assert!(left.contains("<nonce>") && !left.contains(&nonce));
    assert!(!first_warning(&r).contains(&nonce));
}

#[test]
fn stderr_noise_in_a_framed_body_is_not_a_finding() {
    let body = "sh: line 2: warning: something odd\nleft: root /x";
    let report = go(&mac(printing(&stdout_at(6, body))));
    assert_eq!(report.left, ["root /x"]);
}

#[test]
fn the_release_script_sends_stderr_into_the_captured_stream() {
    let r = mac(quiet());
    go(&r);
    assert!(scripts(&r).first().unwrap().starts_with("exec 2>&1\n"));
}

#[test]
fn each_step_gets_its_own_deadline() {
    let r = mac(quiet());
    go(&r);
    let secs: Vec<u64> = r.driver.execs().iter().map(|e| e.2 .0.as_secs()).collect();
    assert_eq!(secs, [40, 120, 30, 120, 30, 30, 30]);
}

/// The `BoxRoot` row of the Task.
fn root_row(r: &Rig) -> crate::store::lease::LeaseRow {
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    rows.into_iter()
        .find(|l| l.kind == LeaseKind::BoxRoot)
        .unwrap()
}

#[test]
fn a_failed_stop_step_stays_on_the_root_row_after_later_steps_succeed() {
    let driver = printing(&stdout_at(0, "agent stuck")).exec_results([exit(1)].map(Ok));
    let r = mac(driver);
    go(&r);
    let row = root_row(&r);
    assert_eq!(row.attempts, 1);
    assert!(row
        .failure
        .as_deref()
        .unwrap()
        .starts_with("stop the agent: agent stuck"));
}

#[test]
fn two_failing_steps_on_one_row_are_both_named() {
    let mut outs = stdout_at(0, "agent stuck");
    if let Some(last) = outs.get_mut(6) {
        *last = framed(6, "left: root /Users/box/moor/demo");
    }
    let driver =
        printing(&outs).exec_results((0..7).map(|i| Ok(exit(i32::from(i == 0 || i == 6)))));
    let r = mac(driver);
    go(&r);
    let row = root_row(&r);
    assert_eq!(row.attempts, 1);
    let failure = row.failure.unwrap();
    assert!(failure.contains("stop the agent: agent stuck"));
    assert!(failure.contains("verify nothing is left: root /Users/box/moor/demo"));
}

#[test]
fn a_second_release_adds_an_attempt_to_the_kept_rows() {
    let r = mac(quiet().fail_release(DriverError::Failed {
        summary: "gone wrong".into(),
    }));
    assert_eq!(root_row(&r).failure, None);
    go(&r);
    assert_eq!(root_row(&r).attempts, 1);
    assert_eq!(root_row(&r).failure, None);
    go(&r);
    assert_eq!(root_row(&r).attempts, 2);
}
