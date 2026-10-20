use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Intent};
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::SETUP_LINE_DEADLINE;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::{BoxHandle, BoxId, ErrorKind, StartReq};

use self::Step::{Cmd, Sync};
use super::moor_yaml::script_base_yaml;
use super::state::WORKING;
use super::{req, world_at, World};
use crate::adapters::clock::SystemClock;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;
use moor_core::domain::box_state::BoxLifecycle;

const LINE_DEADLINE: Duration = SETUP_LINE_DEADLINE;

pub(super) fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// Starts `demo` in a repo whose `.moor.yaml` is `yaml`.
pub(super) fn start_with(yaml: &str, driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(yaml));
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Starts `demo` like [`start_with`] with `git config` answering the given author.
pub(super) fn start_as(
    author: (&str, &str),
    yaml: &str,
    driver: FakeDriver,
) -> (World, moor_core::view::StartView) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let w = world_at(driver, repo.path());
    let ok = |v: &str| Reply::Ok(format!("{v}\n"));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.name"], ok(author.0));
    w.proc
        .on(Tool::Git, &["config", "--get", "user.email"], ok(author.1));
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Whether `argv` is the exec that only syncs the checkout into the Box.
fn is_sync(argv: &[String]) -> bool {
    argv == ["git", "rev-parse", "--git-dir"]
}

/// Every Box exec except those that make or remove the Proxy-credential directory.
pub(super) fn execs(w: &World) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
    let is_cred_dir = |a: &[String]| {
        matches!(a.first().map(String::as_str), Some("mkdir" | "rm"))
            && a.iter().any(|x| x.contains(".moor-cred-"))
    };
    (w.driver.execs().into_iter())
        .filter(|e| !is_cred_dir(&e.1.argv))
        .collect()
}

/// Every exec except the checkout sync.
pub(super) fn setup_execs(w: &World) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
    (execs(w).into_iter())
        .filter(|e| !is_sync(&e.1.argv))
        .collect()
}

/// A scripted exec result: the checkout sync, or a command exec that stages Proxy credentials.
pub(super) enum Step {
    Sync(Result<ExecOutcome, DriverError>),
    Cmd(Result<ExecOutcome, DriverError>),
}

/// The driver results for `steps`: each `Cmd` is preceded by the Ok for its credential-directory
/// `mkdir`, and followed by the Ok for the cleanup `rm` when it errs or is interrupted.
pub(super) fn scripted(
    steps: impl IntoIterator<Item = Step>,
) -> Vec<Result<ExecOutcome, DriverError>> {
    let wrap = |step: Step| match step {
        Step::Sync(r) => vec![r],
        Step::Cmd(r) => {
            let rm = matches!(r, Err(_) | Ok(ExecOutcome::Interrupted));
            let tail = rm.then(|| Ok(exit(0)));
            [Some(Ok(exit(0))), Some(r), tail]
                .into_iter()
                .flatten()
                .collect()
        }
    };
    steps.into_iter().flat_map(wrap).collect()
}

/// Peels the Proxy wrapper (`sh -c <prelude> sh`) off an argv.
pub(super) fn unwrapped(argv: Vec<String>) -> Vec<String> {
    match argv.as_slice() {
        [_, _, _, sh, rest @ ..] if sh == "sh" => rest.to_vec(),
        _ => argv,
    }
}

/// The argv of every exec except the checkout sync, wrapper peeled.
pub(super) fn argvs(w: &World) -> Vec<Vec<String>> {
    setup_execs(w)
        .into_iter()
        .map(|e| unwrapped(e.1.argv))
        .collect()
}

/// Whether `argv` is the batched script that sets the Box's git identity.
pub(super) fn is_identity(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh")
        && argv
            .get(2)
            .is_some_and(|s| s.contains("git config --global"))
}

pub(super) fn is_setup_line(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh") && !is_identity(argv)
}

pub(super) fn start_req() -> StartReq {
    req("demo")
}

/// The argv that runs `steps` as the setup script.
pub(super) fn script_argv(steps: &[&str]) -> Vec<String> {
    let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
    ["sh", "-c", &setup_script(&steps)]
        .map(str::to_owned)
        .to_vec()
}

pub(super) fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

pub(super) fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

pub(super) fn failed_at_set(w: &World, v: &moor_core::view::StartView) -> bool {
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    row.unwrap().unwrap().setup_failed_at.is_some()
}

#[test]
fn setup_lines_run_in_order_in_the_box_workdir() {
    let (w, v) = start_with("setup: [a, b]\n", FakeDriver::new());
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(v.setup.line, None);
    let seen: Vec<_> = setup_execs(&w)
        .into_iter()
        .map(|(h, spec, _)| {
            let cwd = spec.cwd.map(|p| p.as_str().to_owned());
            (h.as_str().to_owned(), unwrapped(spec.argv), cwd)
        })
        .collect();
    let want = (
        "box-1".to_owned(),
        script_argv(&["a", "b"]),
        Some("/work/cbx_1/demo".to_owned()),
    );
    assert_eq!(seen, [want]);
    let calls = w.driver.box_calls();
    assert_eq!(calls.iter().filter(|c| *c == "batch").count(), 1);
}

#[test]
fn setup_runs_under_one_allowance_per_line() {
    let (w, _) = start_with("setup: [a, b]\n", FakeDriver::new());
    let deadlines: Vec<Deadline> = setup_execs(&w).into_iter().map(|e| e.2).collect();
    assert_eq!(deadlines, [Deadline(LINE_DEADLINE * 2)]);
}

#[test]
fn the_checkout_syncs_in_its_own_exec_and_setup_runs_without_syncing() {
    let (w, v) = start_with("setup: [a, b]\nbox: {size: large}\n", FakeDriver::new());
    let wt =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(v.boxed.id))
            .unwrap()
            .unwrap()
            .worktree;
    let mut execs = execs(&w);
    assert_eq!(execs.len(), 2, "a sync exec and one setup exec");
    let (_, setup, _) = execs.remove(1);
    let (_, sync, _) = execs.remove(0);
    assert_eq!(sync.argv, ["git", "rev-parse", "--git-dir"]);
    assert_eq!(sync.sync, SyncMode::Full);
    assert_eq!(sync.host_cwd, wt.clone().map(Into::into));
    assert_eq!(sync.class.as_deref(), Some("large"));
    assert_eq!(setup.sync, SyncMode::None);
    assert_eq!(setup.host_cwd, wt.map(Into::into));
    assert_eq!(setup.class.as_deref(), Some("large"));
}

#[test]
fn a_failing_setup_leaves_a_working_task_and_runs_nothing_more() {
    let driver = FakeDriver::new()
        .exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]))
        .copy_text("moor setup: $ a\nmoor setup: $ b\n");
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(setup_execs(&w).len(), 1);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    let id = BoxId::new(v.boxed.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    assert_eq!(failed.payload.get("line"), Some(&serde_json::json!("b")));
    assert!(kinds(&w, id).contains(&"working".to_owned()));
}

#[test]
fn a_failing_setup_names_the_step_tail_and_box_in_json_and_text() {
    let driver = FakeDriver::new()
        .exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]))
        .copy_text("moor setup: $ a\nboom: no such file\n");
    let (_, v) = start_with("setup: [a]\n", driver);
    assert_eq!(v.failed_step.as_deref(), Some("setup"));
    assert_eq!(v.handle.as_deref(), Some("box-1"));
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json.pointer("/failed_step"), Some(&"setup".into()));
    assert_eq!(json.pointer("/handle"), Some(&"box-1".into()));
    assert_eq!(
        json.pointer("/setup/tail"),
        Some(&"moor setup: $ a\nboom: no such file".into())
    );
    let out = crate::cli::render(
        &moor_core::Response::ok("start", Some(json)),
        &crate::cli::GlobalOpts::default(),
    )
    .stdout;
    assert!(out.starts_with("started demo (#1): ready/working, box ready, setup failed\n"));
    assert!(out.contains(
        "setup failed in Box box-1:\n  moor setup: $ a\n  boom: no such file\nclose it with: moor close demo\n"
    ));
    assert!(out.contains("open it with: moor open demo\n"));
}

#[test]
fn a_clean_setup_names_no_failed_step() {
    let driver = FakeDriver::new().exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(0)))]));
    let (_, v) = start_with("setup: [a]\n", driver);
    assert_eq!((v.failed_step, v.handle), (None, None));
}

#[test]
fn a_failed_setup_copies_its_log_out_of_the_box_into_the_worktree() {
    let driver = FakeDriver::new().exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]));
    let (w, v) = start_with("setup: [a]\n", driver);
    let wt =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(v.boxed.id))
            .unwrap()
            .unwrap()
            .worktree;
    let wt = std::path::PathBuf::from(wt.unwrap());
    let (_, from, to, host_cwd) = w.driver.copies().remove(0);
    assert_eq!(from.as_str(), "/work/cbx_1/demo/.moor/setup.log");
    assert_eq!(to, wt.join(".moor/setup.log"));
    assert_eq!(host_cwd, wt);
}

#[test]
fn a_failing_setup_marks_the_task_and_a_later_clean_setup_clears_it() {
    let driver = FakeDriver::new().exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]));
    let (w, v) = start_with("setup: [a]\n", driver);
    assert!(!v.setup.ok);
    assert!(failed_at_set(&w, &v));
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
    // A re-provision reads `.moor.yaml` from the worktree, not the invocation repo.
    let wt = store.box_row(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
    store.apply(gone).unwrap();
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!(again.boxed.id, v.boxed.id);
    assert!(again.setup.ran && again.setup.ok);
    assert!(!failed_at_set(&w, &again));
}

#[test]
fn no_setup_lines_means_setup_did_not_run() {
    let (w, v) = start_with("env: {A: a}\n", FakeDriver::new());
    assert!(!v.setup.ran);
    let execs = execs(&w);
    assert_eq!(execs.len(), 1);
    assert_eq!(
        execs.first().expect("one exec").1.argv,
        ["git", "rev-parse", "--git-dir"]
    );
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
}

#[test]
fn setup_env_is_the_repo_env_plus_the_proxy_rule_env() {
    let yaml = "setup: [a]\nenv: {ZED: z, ALPHA: a}\n";
    let (w, _) = start_with(yaml, FakeDriver::new());
    let execs = setup_execs(&w).into_iter();
    let env = execs.flat_map(|e| e.1.env).map(|(k, v)| format!("{k}={v}"));
    let env: Vec<_> = env.collect();
    assert!(env.starts_with(&["ALPHA=a".to_owned(), "ZED=z".to_owned()]));
    assert!(env.contains(&"CLAUDE_CODE_OAUTH_TOKEN=moor-proxy".to_owned()));
}

#[test]
fn a_timed_out_setup_counts_as_a_failing_line() {
    let driver = FakeDriver::new()
        .exec_results(scripted([
            Sync(Ok(exit(0))),
            Cmd(Err(DriverError::Timeout(LINE_DEADLINE))),
        ]))
        .copy_text("moor setup: $ a\nmoor setup: $ b\n");
    let (w, v) = start_with("setup: [a, b, c]\n", driver);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(v.setup.line.as_deref(), Some("b"));
    assert_eq!(setup_execs(&w).len(), 1);
    assert!(warning_codes(&w).contains(&"setup_failed".to_owned()));
    assert!(failed_at_set(&w, &v));
}

#[test]
fn a_failing_setup_on_a_re_provisioned_working_task_records_the_failing_line() {
    let driver = FakeDriver::new().exec_results(scripted([
        Sync(Ok(exit(0))),
        Cmd(Ok(exit(0))),
        Sync(Ok(exit(0))),
        Cmd(Ok(exit(1))),
    ]));
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
    let wt = store.box_row(id).unwrap().unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [a]\n").unwrap();
    script_base_yaml(&w, Some("setup: [a]\n"));
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    let again = run(&w.cx, &start_req()).unwrap();
    assert_eq!((again.boxed.lifecycle, again.boxed.session_state), WORKING);
    assert_eq!(again.setup.line.as_deref(), Some("a"));
    let events = store.events(id, 50).unwrap();
    let lines: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_failed")
        .map(|e| e.payload.get("line").cloned())
        .collect();
    assert_eq!(lines, [Some(serde_json::json!("a"))]);
}

#[test]
fn setup_holds_the_setup_intent_and_clears_it_after() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [a]\n").unwrap();
    let w = world_at(FakeDriver::new(), repo.path());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join("moor.db"));
    w.driver.on_exec(move || {
        let store = Store::open(&path, Arc::new(SystemClock::real())).unwrap();
        let row = store.box_row(BoxId::new(1)).unwrap().unwrap();
        sink.lock().unwrap().push(row.intent);
    });
    let v = run(&w.cx, &start_req()).unwrap();
    let pid = std::process::id();
    // The checkout sync, the credential-directory mkdir and the setup script all run under it.
    assert_eq!(*seen.lock().unwrap(), [Some(Intent::Setup { pid }); 3]);
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

/// Like [`failed_setup_then`] with the first setup run on `driver`.
pub(super) fn failed_setup_with(yaml: &str, driver: FakeDriver) -> (World, BoxId) {
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = BoxId::new(v.boxed.id);
    let wt = w.cx.store().unwrap().box_row(id).unwrap().unwrap().worktree;
    let wt = wt.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), yaml).unwrap();
    script_base_yaml(&w, Some(yaml));
    (w, id)
}

#[test]
fn an_unset_author_runs_setup_without_touching_git_config() {
    let (w, v) = start_with("setup: [a, b]\n", FakeDriver::new());
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w), [script_argv(&["a", "b"])]);
}

#[test]
fn a_failing_identity_exec_warns_and_setup_still_runs() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w).last(), Some(&script_argv(&["a"])));
    assert!(warning_codes(&w).contains(&"identity_failed".to_owned()));
}

#[test]
fn a_setup_run_records_its_duration_on_a_setup_finished_event() {
    let driver = FakeDriver::new().exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(0)))]));
    let (w, v) = start_with("setup: [a]\n", driver);
    assert_eq!(v.setup.ms, Some(0));
    let id = BoxId::new(v.boxed.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let got: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_finished")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(got, [serde_json::json!({ "ms": 0, "ok": true })]);
}

#[test]
fn a_failed_setup_records_its_duration_too() {
    let driver = FakeDriver::new().exec_results(scripted([Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]));
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = BoxId::new(v.boxed.id);
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let got: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "setup_finished")
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(got, [serde_json::json!({ "ms": 0, "ok": false })]);
}

/// Starts `demo` on a driver whose first execs give `results`; the engine records launches.
fn start_recording_launches(
    yaml: &str,
    results: [Step; 2],
) -> (
    World,
    Arc<FakeEngine>,
    Result<moor_core::view::StartView, crate::error::CommandError>,
) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let mut w = world_at(
        FakeDriver::new().exec_results(scripted(results)),
        repo.path(),
    );
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let with_goal = StartReq {
        goal: Some("ship it".into()),
        ..start_req()
    };
    let got = run(&w.cx, &with_goal);
    (w, engine, got)
}

#[test]
fn a_failed_checkout_sync_aborts_the_start_without_launching_the_agent() {
    for yaml in ["env: {A: a}\n", "setup: [a, b]\n"] {
        let (w, engine, got) =
            start_recording_launches(yaml, [Sync(Ok(exit(1))), Cmd(Ok(exit(0)))]);
        let err = got.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        let msg = err.error.to_string();
        assert!(
            msg.contains("syncing the checkout into the Box failed"),
            "{msg}"
        );
        assert!(engine.launches().is_empty());
        let id = BoxId::new(1);
        let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
        assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
        assert!(row.intent.is_none() && row.setup_failed_at.is_some());
        assert!(kinds(&w, id).contains(&"setup_failed".to_owned()));
        let argvs: Vec<_> = execs(&w).into_iter().map(|e| e.1.argv).collect();
        assert_eq!(
            argvs,
            [vec![
                "git".to_owned(),
                "rev-parse".to_owned(),
                "--git-dir".to_owned()
            ]]
        );
    }
}

#[test]
fn a_failing_setup_command_after_a_good_sync_still_launches_the_agent() {
    let (w, engine, got) =
        start_recording_launches("setup: [a]\n", [Sync(Ok(exit(0))), Cmd(Ok(exit(1)))]);
    let v = got.unwrap();
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(engine.launches().len(), 1);
    let syncs: Vec<_> = execs(&w).into_iter().map(|e| e.1.sync).collect();
    assert_eq!(syncs, [SyncMode::Full, SyncMode::None]);
}
