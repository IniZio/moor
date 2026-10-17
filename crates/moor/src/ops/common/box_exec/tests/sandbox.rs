//! Tests for the macOS Box exec under the sandbox.

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::engine::{EngineError, Sandboxed};
use moor_core::{BoxPath, TaskName};

use super::{argvs, boxed_proxy, cfg, rig, rig_driver, Rig};
use crate::ops::common::box_exec::{prepare, BoxExec};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

const WORKDIR: &str = "/Users/box/moor/demo/work/repo";
const HOME: &str = "/Users/box/moor/demo/home";

fn sandboxed() -> Sandboxed {
    Sandboxed {
        lines: vec!["harden-line".to_owned()],
        prefix: vec!["nono".to_owned(), "run".to_owned(), "--".to_owned()],
    }
}

fn mac_rig(dir: &Path, engine: FakeEngine) -> (Rig, Arc<FakeEngine>) {
    mac_rig_with(dir, FakeDriver::new(), engine)
}

fn mac_rig_with(dir: &Path, driver: FakeDriver, engine: FakeEngine) -> (Rig, Arc<FakeEngine>) {
    let mut r = rig_driver(dir, crate::testing::cx::fake_config(), driver);
    r.cx.store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: TaskName::parse("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap();
    let engine = Arc::new(engine);
    r.cx = r.cx.with_engine(Arc::clone(&engine) as Arc<_>);
    (r, engine)
}

fn run(r: &Rig, dir: &Path, proxied: bool, purpose: Purpose) -> Result<BoxExec, String> {
    let workdir = BoxPath::restore(WORKDIR).unwrap();
    let p = boxed_proxy();
    prepare(
        &r.cx,
        &r.site(Some(&workdir), dir),
        proxied.then_some(&p),
        &cfg(),
        purpose,
    )
}

fn script(e: &BoxExec) -> String {
    script_of(&e.argv(vec!["setup-cmd".to_owned()]))
}

fn script_of(argv: &[String]) -> String {
    argv.get(2).cloned().unwrap()
}

#[test]
fn a_mac_exec_hands_the_task_env_to_the_engine_and_exports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (r, engine) = mac_rig(
        dir.path(),
        FakeEngine::new().sandbox_result(Ok(Some(sandboxed()))),
    );
    let e = run(&r, dir.path(), true, Purpose::Run).unwrap();
    let [req] = engine.sandbox_reqs().try_into().unwrap();
    assert!(req.set_vars.contains(&("HOME".to_owned(), HOME.to_owned())));
    assert_eq!(Some(&req.cred), e.staged.as_ref());
    assert!(req
        .cred
        .as_str()
        .starts_with("/Users/box/moor/demo/launch/"));
    assert_eq!(req.workdir.as_str(), WORKDIR);
    assert_eq!(req.task.as_str(), "demo");
    assert!(!req.report);
    assert!(e.vars().is_empty());
    assert_sandboxed_argv(&e);
}

fn assert_sandboxed_argv(e: &BoxExec) {
    let argv = e.argv(vec!["make".to_owned(), "test".to_owned()]);
    assert_eq!(argv.get(..2), Some(&["sh".to_owned(), "-c".to_owned()][..]));
    let script = script_of(&argv);
    assert!(!script.contains(HOME), "{script}");
    let harden = script.find("harden-line").unwrap();
    assert!(script.rfind(r#""$@""#).unwrap() > harden, "{script}");
    assert_eq!(
        argv.get(4..),
        Some(&["nono", "run", "--", "make", "test"].map(str::to_owned)[..])
    );
}

#[test]
fn only_setup_asks_for_the_report_and_redirects_stderr_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let engine = FakeEngine::new()
        .sandbox_result(Ok(Some(sandboxed())))
        .sandbox_result(Ok(Some(sandboxed())));
    let (r, engine) = mac_rig(dir.path(), engine);
    let setup = run(&r, dir.path(), true, Purpose::Setup { lines: 1 }).unwrap();
    let teardown = run(&r, dir.path(), true, Purpose::Teardown).unwrap();
    let reports: Vec<bool> = engine.sandbox_reqs().iter().map(|q| q.report).collect();
    assert_eq!(reports, [true, false]);
    let text = script(&setup);
    let mkdir = format!(
        "mkdir -p '{WORKDIR}/.moor' && date '+%Y-%m-%d %H:%M:%S' > '{WORKDIR}/.moor/setup.sandbox.start' && echo $$ >> '{WORKDIR}/.moor/setup.sandbox.start' && exec 2>'{WORKDIR}/.moor/setup.sandbox.json'"
    );
    assert!(text.contains(&mkdir), "{text}");
    assert!(text.find(&mkdir) < text.find("harden-line"), "{text}");
    assert!(!script(&teardown).contains("exec 2>"));
    assert!(!script(&teardown).contains("setup.sandbox.start"));
}

#[test]
fn without_a_proxy_a_cred_dir_is_still_created_and_the_sandbox_used() {
    let dir = tempfile::tempdir().unwrap();
    let (r, engine) = mac_rig(
        dir.path(),
        FakeEngine::new().sandbox_result(Ok(Some(sandboxed()))),
    );
    let e = run(&r, dir.path(), false, Purpose::Run).unwrap();
    let [req] = engine.sandbox_reqs().try_into().unwrap();
    let [_prep, mkdir] = argvs(&r).try_into().unwrap();
    assert_eq!(mkdir.first().map(String::as_str), Some("mkdir"));
    assert_eq!(mkdir.last().map(String::as_str), Some(req.cred.as_str()));
    assert!(e.vars().is_empty());
    assert!(script(&e).contains("trap"));
}

#[test]
fn an_engine_error_fails_prepare_and_removes_the_cred_dir() {
    let dir = tempfile::tempdir().unwrap();
    let err = EngineError::Failed {
        step: "profile",
        message: "no nono".to_owned(),
    };
    let (r, engine) = mac_rig(dir.path(), FakeEngine::new().sandbox_result(Err(err)));
    let got = run(&r, dir.path(), true, Purpose::Run);
    assert!(got.err().unwrap().contains("no nono"));
    let [req] = engine.sandbox_reqs().try_into().unwrap();
    let last = argvs(&r).pop().unwrap();
    assert_eq!(
        last,
        ["rm", "-rf", req.cred.as_str()].map(str::to_owned).to_vec()
    );
}

#[test]
fn no_sandbox_keeps_the_exported_task_env() {
    let dir = tempfile::tempdir().unwrap();
    let (r, _) = mac_rig(dir.path(), FakeEngine::new());
    let e = run(&r, dir.path(), true, Purpose::Run).unwrap();
    assert!(e.vars().contains(&("HOME".to_owned(), HOME.to_owned())));
}

#[test]
fn a_failed_cred_dir_create_on_a_mac_box_without_a_proxy_asks_no_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 1 }),
    ]);
    let (r, engine) = mac_rig_with(dir.path(), driver, FakeEngine::new());
    let got = run(&r, dir.path(), false, Purpose::Run);
    assert!(got.err().unwrap().contains("already exists"));
    assert!(engine.sandbox_reqs().is_empty());
}

#[test]
fn a_linux_box_never_asks_for_a_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = rig(dir.path());
    let engine = Arc::new(FakeEngine::new().sandbox_result(Ok(Some(sandboxed()))));
    r.cx = r.cx.with_engine(Arc::clone(&engine) as Arc<_>);
    let e = prepare(
        &r.cx,
        &r.site(Some(&r.workdir), dir.path()),
        None,
        &cfg(),
        Purpose::Run,
    )
    .unwrap();
    assert!(engine.sandbox_reqs().is_empty());
    assert!(!e.vars().is_empty());
}
