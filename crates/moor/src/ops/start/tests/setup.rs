use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::boxscript::setup::setup_script;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Intent};
use moor_core::exec_env::SETUP_LINE_DEADLINE;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::forge::{GitAuthor, RepoAt};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, ErrorKind, StartReq};

use super::state::WORKING;
use super::{repo, repo_req, script_base_yaml, world, World, BASE};
use crate::adapters::clock::SystemClock;
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::driver::{is_clone_exec, FakeDriver};
use crate::testing::engine::FakeEngine;
use crate::testing::forge::repo_at;
use moor_core::domain::box_state::BoxLifecycle;

const LINE_DEADLINE: Duration = SETUP_LINE_DEADLINE;

pub(super) fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

/// Starts `demo` of a repo whose `.moor.yaml` is `yaml`.
pub(super) fn start_with(yaml: &str, driver: FakeDriver) -> (World, moor_core::view::StartView) {
    let w = world(driver);
    script_base_yaml(&w, Some(yaml));
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Starts `demo` like [`start_with`] with the forge reporting the given author.
pub(super) fn start_as(
    author: (&str, &str),
    yaml: &str,
    driver: FakeDriver,
) -> (World, moor_core::view::StartView) {
    let w = world(driver);
    let author = GitAuthor {
        name: author.0.to_owned(),
        email: author.1.to_owned(),
    };
    w.forge.answer(
        repo(),
        Ok(RepoAt {
            author,
            ..repo_at(BASE, Some(yaml))
        }),
    );
    let v = run(&w.cx, &start_req()).unwrap();
    (w, v)
}

/// Every exec except the clone.
pub(super) fn setup_execs(w: &World) -> Vec<(BoxHandle, ExecSpec, Deadline)> {
    (w.driver.execs().into_iter())
        .filter(|e| !is_clone_exec(&e.1))
        .collect()
}

/// Peels the Proxy wrapper (`sh -c <prelude> sh`) off an argv.
pub(super) fn unwrapped(argv: Vec<String>) -> Vec<String> {
    match argv.as_slice() {
        [_, _, _, sh, rest @ ..] if sh == "sh" => rest.to_vec(),
        _ => argv,
    }
}

/// The argv of every exec except the clone, wrapper peeled.
pub(super) fn argvs(w: &World) -> Vec<Vec<String>> {
    setup_execs(w)
        .into_iter()
        .map(|e| unwrapped(e.1.argv))
        .collect()
}

/// Whether `argv` is the clone exec's script that sets the Box's git identity.
pub(super) fn is_identity(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh")
        && argv
            .last()
            .is_some_and(|s| s.contains("git config --global"))
}

pub(super) fn is_setup_line(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("sh") && !is_identity(argv)
}

pub(super) fn start_req() -> StartReq {
    repo_req("demo")
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
        Some("/work/cbx_1/repo".to_owned()),
    );
    assert_eq!(seen, [want]);
    let calls = w.driver.box_calls();
    assert!(calls.iter().all(|c| c != "batch"), "{calls:?}");
}

#[test]
fn setup_runs_under_one_allowance_per_line() {
    let (w, _) = start_with("setup: [a, b]\n", FakeDriver::new());
    let deadlines: Vec<Deadline> = setup_execs(&w).into_iter().map(|e| e.2).collect();
    assert_eq!(deadlines, [Deadline(LINE_DEADLINE * 2)]);
}

#[test]
fn the_repo_clones_in_its_own_exec_before_setup() {
    let (w, _) = start_with("setup: [a, b]\n", FakeDriver::new());
    let mut execs = w.driver.execs();
    assert_eq!(execs.len(), 2, "a clone exec and one setup exec");
    let (_, clone, _) = execs.remove(0);
    assert!(is_clone_exec(&clone));
    assert!(
        clone.argv.last().unwrap().contains("git clone"),
        "{:?}",
        clone.argv
    );
}

#[test]
fn a_failing_setup_leaves_a_working_box_and_runs_nothing_more() {
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
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
        .exec_results([Ok(exit(0)), Ok(exit(1))])
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
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0))]);
    let (_, v) = start_with("setup: [a]\n", driver);
    assert_eq!((v.failed_step, v.handle), (None, None));
}

#[test]
fn a_failed_setup_copies_its_log_into_an_owner_only_dir_under_the_state_dir() {
    use std::os::unix::fs::PermissionsExt as _;
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
        .copy_text("moor setup: $ a\n");
    let (w, v) = start_with("setup: [a]\n", driver);
    let dir = w.dir.path().join("logs").join(v.boxed.id.to_string());
    let (_, from, to) = w.driver.copies().remove(0);
    assert_eq!(from.as_str(), "/work/cbx_1/repo/.moor/setup.log");
    assert_eq!(to, dir.join("setup.log"));
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(dir.parent().unwrap()), 0o700);
    assert_eq!(mode(&to), 0o600);
}

/// Every file under `dir`, relative to it.
fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(
                files_under(&path)
                    .into_iter()
                    .map(|f| format!("{}/{f}", entry.file_name().to_string_lossy())),
            );
        } else {
            out.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    out
}

#[test]
fn setup_writes_only_its_log_under_the_state_logs_dir() {
    for second in [Ok(exit(0)), Ok(exit(1))] {
        let driver = FakeDriver::new()
            .exec_results([Ok(exit(0)), second])
            .copy_text("moor setup: $ a\n");
        let (w, v) = start_with("setup: [a]\n", driver);
        let logs = w.dir.path().join("logs");
        assert_eq!(files_under(&logs), [format!("{}/setup.log", v.boxed.id)]);
    }
}

#[test]
fn a_failing_setup_marks_the_box_and_a_later_clean_setup_clears_it() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    assert!(!v.setup.ok);
    assert!(failed_at_set(&w, &v));
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
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
    let execs = w.driver.execs();
    assert_eq!(execs.len(), 1);
    assert!(
        is_clone_exec(&execs.first().expect("one exec").1),
        "{execs:?}"
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
        .exec_results([Ok(exit(0)), Err(DriverError::Timeout(LINE_DEADLINE))])
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
fn a_failing_setup_on_a_re_provisioned_working_box_records_the_failing_line() {
    let driver =
        FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
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
    let w = world(FakeDriver::new());
    script_base_yaml(&w, Some("setup: [a]\n"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join("moor.db"));
    w.driver.on_exec(move || {
        let store = Store::open(&path, Arc::new(SystemClock::real())).unwrap();
        let row = store.box_row(BoxId::new(1)).unwrap().unwrap();
        sink.lock().unwrap().push(row.intent);
    });
    let v = run(&w.cx, &start_req()).unwrap();
    let pid = std::process::id();
    // The clone and the setup script both run under it.
    assert_eq!(*seen.lock().unwrap(), [Some(Intent::Setup { pid }); 2]);
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
    assert_eq!(row.unwrap().unwrap().intent, None);
}

/// Starts `demo` like [`start_with`] and gives back its Box id; `driver`'s results decide
/// how its first setup ends.
pub(super) fn failed_setup_with(yaml: &str, driver: FakeDriver) -> (World, BoxId) {
    let (w, v) = start_with(yaml, driver);
    (w, BoxId::new(v.boxed.id))
}

#[test]
fn an_unset_author_runs_setup_without_touching_git_config() {
    let (w, v) = start_as(("", ""), "setup: [a, b]\n", FakeDriver::new());
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w), [script_argv(&["a", "b"])]);
    assert!(setup_execs(&w).iter().all(|e| !is_identity(&e.1.argv)));
}

#[test]
fn a_failing_identity_key_warns_and_setup_still_runs() {
    let driver = FakeDriver::new().exec_stdout(["user.name=1\nuser.email=0\n"]);
    let (w, v) = start_as(("Ann Dev", "a@x.io"), "setup: [a]\n", driver);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && v.setup.ok);
    assert_eq!(argvs(&w).last(), Some(&script_argv(&["a"])));
    assert!(warning_codes(&w).contains(&"identity_failed".to_owned()));
}

#[test]
fn a_setup_run_records_its_duration_on_a_setup_finished_event() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0))]);
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
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
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
    results: [Result<ExecOutcome, DriverError>; 2],
) -> (
    World,
    Arc<FakeEngine>,
    Result<moor_core::view::StartView, crate::error::CommandError>,
) {
    let mut w = world(FakeDriver::new().exec_results(results));
    script_base_yaml(&w, Some(yaml));
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    let with_prompt = StartReq {
        prompt: Some("ship it".into()),
        ..start_req()
    };
    let got = run(&w.cx, &with_prompt);
    (w, engine, got)
}

#[test]
fn a_failed_clone_aborts_the_start_without_launching_the_agent() {
    for yaml in ["env: {A: a}\n", "setup: [a, b]\n"] {
        let (w, engine, got) = start_recording_launches(yaml, [Ok(exit(1)), Ok(exit(0))]);
        let err = got.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        let msg = err.error.to_string();
        assert!(
            msg.contains("cloning the repo into the Box failed"),
            "{msg}"
        );
        assert!(engine.launches().is_empty());
        let id = BoxId::new(1);
        let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
        assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
        assert!(row.intent.is_none() && row.setup_failed_at.is_some());
        assert!(kinds(&w, id).contains(&"setup_failed".to_owned()));
        let execs = w.driver.execs();
        assert_eq!(execs.len(), 1, "only the clone ran");
        assert!(execs.first().is_some_and(|e| is_clone_exec(&e.1)));
    }
}

#[test]
fn a_failing_setup_command_after_a_good_clone_still_launches_the_agent() {
    let (w, engine, got) = start_recording_launches("setup: [a]\n", [Ok(exit(0)), Ok(exit(1))]);
    let v = got.unwrap();
    assert!(v.setup.ran && !v.setup.ok);
    assert_eq!(engine.launches().len(), 1);
    let clones: Vec<_> = w
        .driver
        .execs()
        .iter()
        .map(|e| is_clone_exec(&e.1))
        .collect();
    assert_eq!(clones, [true, false]);
}
