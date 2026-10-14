use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::config::{DotfilesConfig, UserConfig};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::domain::decision::Intent;
use moor_core::domain::task::TaskState;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::view::{DotfilesView, StartView};
use moor_core::seam::clock::Ts;
use moor_core::{ErrorKind, StartReq, TaskId};

use super::{hold, req, world_with, World};
use crate::ops::start::run;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;

const TAR_SCRIPT: &str = "exec tar -C \"$1\" -czhf - .";
const ARCHIVE: &str = "/tmp/moor-dotfiles-1.tgz";

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

fn goal_req() -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    }
}

/// The Box script text for `archive` with install line `install`.
fn script(archive: &str, install: &str) -> String {
    let qa = quote(archive);
    format!(
        "set -e; rm -rf \"$HOME/.moor-dotfiles.moor-tmp\"; mkdir -p \"$HOME/.moor-dotfiles.moor-tmp\"; \
         tar -xzf {qa} -C \"$HOME/.moor-dotfiles.moor-tmp\"; rm -f {qa}; \
         rm -rf \"$HOME/.moor-dotfiles\"; mv \"$HOME/.moor-dotfiles.moor-tmp\" \"$HOME/.moor-dotfiles\"; \
         cd \"$HOME/.moor-dotfiles\"; {install}"
    )
}

const DEFAULT_INSTALL: &str = "if [ -f install.sh ]; then MOOR_DOTFILES=1 bash ./install.sh; fi";

/// A world over a repo with `yaml`, dotfiles at `dir` (when given), and an engine.
fn dotfiles_world(
    yaml: &str,
    dir: Option<&Path>,
    install: Option<&str>,
    driver: FakeDriver,
) -> (World, Arc<FakeEngine>, tempfile::TempDir) {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let config = UserConfig {
        dotfiles: dir.map(|d| DotfilesConfig {
            path: d.to_path_buf(),
            install: install.map(str::to_owned),
        }),
        ..Default::default()
    };
    let mut w = world_with(driver, repo.path(), config);
    w.proc.on(
        Tool::Sh,
        &["-c", TAR_SCRIPT],
        Reply::Bytes(vec![0x1f, 0x8b, 1, 2, 3]),
    );
    let engine = Arc::new(FakeEngine::new());
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine, repo)
}

/// Execs that were the dotfiles step.
fn dotfiles_execs(w: &World) -> Vec<(Vec<String>, Deadline, SyncMode)> {
    w.driver
        .execs()
        .into_iter()
        .filter(|e| e.1.sync == SyncMode::None)
        .map(|e| (e.1.argv, e.2, e.1.sync))
        .collect()
}

fn started(w: &World) -> StartView {
    run(&w.cx, &goal_req()).unwrap()
}

/// The one upload: the scripted tar bytes at the `.tgz` path, owner-only.
fn assert_archive_put(w: &World) {
    let puts = w.driver.puts();
    assert_eq!(puts.len(), 1);
    let (handle, path, bytes, mode, _) = puts.first().unwrap();
    assert_eq!(handle.as_str(), "box-1");
    assert_eq!(path.as_str(), ARCHIVE);
    assert_eq!(bytes, &[0x1f, 0x8b, 1, 2, 3]);
    assert_eq!(*mode, 0o600);
}

/// The second exec (after setup) is the dotfiles script.
fn assert_dotfiles_exec(w: &World) {
    let want = ["sh", "-c", &script(ARCHIVE, DEFAULT_INSTALL)].map(str::to_owned);
    let execs = w.driver.execs();
    assert_eq!(execs.len(), 2);
    let (_, spec, deadline) = execs.last().unwrap();
    assert_eq!(spec.argv, want);
    assert_eq!(spec.sync, SyncMode::None);
    assert_eq!(spec.cwd, None);
    assert!(spec.env.is_empty());
    assert_eq!(*deadline, Deadline(Duration::from_mins(15)));
}

/// The only `sh` proc call is the host tar of `dir`.
fn assert_tar_call(w: &World, dir: &Path) {
    let tars: Vec<_> = w
        .proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Sh)
        .map(|c| c.args)
        .collect();
    let dir = dir.to_str().unwrap();
    assert_eq!(tars, [vec!["-c", TAR_SCRIPT, "sh", dir]]);
}

#[test]
fn dotfiles_run_after_setup_and_before_the_agent_launches() {
    let src = tempfile::tempdir().unwrap();
    let (w, engine, _repo) =
        dotfiles_world("setup: [a]\n", Some(src.path()), None, FakeDriver::new());
    let v = started(&w);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);

    assert_archive_put(&w);
    assert_dotfiles_exec(&w);
    assert_tar_call(&w, src.path());

    let calls = w.driver.calls();
    assert_eq!(
        calls,
        ["acquire", "exec box-1", "put_file box-1", "exec box-1"]
    );
    assert!(v.dotfiles.ran && v.dotfiles.ok);
    assert_eq!(v.dotfiles.ms, Some(0));
}

#[test]
fn a_configured_install_script_with_a_space_is_quoted_and_guarded() {
    let src = tempfile::tempdir().unwrap();
    let (w, _, _repo) = dotfiles_world(
        "egress: [example.com]\n",
        Some(src.path()),
        Some("my setup.sh"),
        FakeDriver::new(),
    );
    started(&w);
    let install = "[ -f './my setup.sh' ] || { echo 'dotfiles install script my setup.sh not found' >&2; exit 1; }; MOOR_DOTFILES=1 bash './my setup.sh'";
    let want = ["sh", "-c", &script(ARCHIVE, install)].map(str::to_owned);
    let argvs: Vec<_> = dotfiles_execs(&w).into_iter().map(|e| e.0).collect();
    assert_eq!(argvs, [want]);
}

/// Writes a 25-line host log for the dotfiles exec of task 1 on each exec.
fn log_writer(w: &World) {
    let log = w.dir.path().join("dotfiles/1.log");
    w.driver.on_exec(move || {
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let text = (1..=25).fold(String::new(), |mut t, n| {
            writeln!(t, "row-{n:02}").unwrap();
            t
        });
        std::fs::write(&log, text).unwrap();
    });
}

/// A started `demo` whose dotfiles exec failed (setup passes first).
fn failed_start() -> (World, Arc<FakeEngine>, tempfile::TempDir, tempfile::TempDir) {
    let src = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1))]);
    let (w, engine, repo) = dotfiles_world("setup: [a]\n", Some(src.path()), None, driver);
    log_writer(&w);
    (w, engine, repo, src)
}

#[test]
fn a_failing_install_stops_start_before_the_agent_with_the_log_tail() {
    let (w, engine, _repo, _src) = failed_start();
    let err = run(&w.cx, &goal_req()).unwrap_err();
    let msg = err.error.to_string();
    assert!(msg.contains("row-25"), "{msg}");
    assert!(msg.contains("row-06"), "{msg}");
    assert!(!msg.contains("row-05"), "{msg}");
    assert!(!msg.contains("row-01"), "{msg}");

    assert_task_kept_in_setup_failed(&w);
    assert!(engine.launches().is_empty());
    assert_event_tail(&w);
    let calls = w.driver.calls();
    let last_exec = calls.iter().rposition(|c| c == "exec box-1").unwrap();
    assert_eq!(
        last_exec,
        calls.len() - 1,
        "nothing after dotfiles: {calls:?}"
    );
}

/// The task is `SetupFailed` and keeps its ready Box.
fn assert_task_kept_in_setup_failed(w: &World) {
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    assert_eq!(row.state, TaskState::SetupFailed);
    assert_eq!(row.boxed.map(|b| b.state), Some(BoxState::Ready));
    assert!(!w.driver.calls().iter().any(|c| c.starts_with("release")));
}

/// The `setup_failed` event carries the log tail only.
fn assert_event_tail(w: &World) {
    let events = w.cx.store().unwrap().events(TaskId::new(1), 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let line = failed.payload.get("line").and_then(|l| l.as_str()).unwrap();
    assert!(
        line.contains("row-25") && !line.contains("row-05"),
        "{line}"
    );
}

#[test]
fn start_after_a_failed_dotfiles_run_re_applies_them_in_the_same_box() {
    let (w, engine, _repo, _src) = failed_start();
    run(&w.cx, &goal_req()).unwrap_err();
    let v = run(&w.cx, &goal_req()).unwrap();
    assert_eq!(v.task.state, TaskState::Working);
    assert_eq!(
        v.dotfiles,
        DotfilesView {
            ran: true,
            ok: true,
            ms: Some(0)
        }
    );
    let puts = w.driver.puts();
    assert_eq!(puts.len(), 2);
    assert!(puts.iter().all(|p| p.0.as_str() == "box-1"));
    assert_eq!(dotfiles_execs(&w).len(), 2);
    assert!(w.driver.execs().iter().all(|e| e.0.as_str() == "box-1"));
    assert_eq!(engine.launches().len(), 1);
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Working);
}

#[test]
fn without_dotfiles_config_nothing_is_packed_uploaded_or_run() {
    let (w, _, _repo) = dotfiles_world("setup: [a]\n", None, None, FakeDriver::new());
    let v = started(&w);
    assert_eq!(v.dotfiles, DotfilesView::default());
    assert!(w.proc.calls().iter().all(|c| c.tool != Tool::Sh));
    assert!(w.driver.puts().is_empty());
    assert!(dotfiles_execs(&w).is_empty());
}

#[test]
fn dotfiles_still_run_when_a_setup_line_fails() {
    let src = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(exit(1)), Ok(exit(0))]);
    let (w, engine, _repo) = dotfiles_world("setup: [a]\n", Some(src.path()), None, driver);
    let v = started(&w);
    assert_eq!(v.task.state, TaskState::Working);
    assert!(v.setup.ran && !v.setup.ok);
    assert!(v.dotfiles.ran && v.dotfiles.ok);
    assert_eq!(w.driver.execs().len(), 2);
    assert_eq!(dotfiles_execs(&w).len(), 1);
    let (_, spec, _) = w.driver.execs().pop().unwrap();
    assert_eq!(spec.sync, SyncMode::None);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn a_retry_is_refused_while_another_op_holds_the_box_lock() {
    let (w, engine, _repo, _src) = failed_start();
    run(&w.cx, &goal_req()).unwrap_err();
    let _held = hold(&w, "sleep");
    let err = run(&w.cx, &goal_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert_eq!(err.error.to_string(), "box busy: sleep in progress");
    assert_eq!(dotfiles_execs(&w).len(), 1);
    assert!(engine.launches().is_empty());
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    assert_eq!(row.state, TaskState::SetupFailed);
    assert!(row.intent.is_none());
}

#[test]
fn a_retry_holds_the_setup_intent_while_the_install_runs() {
    let (w, _, _repo, _src) = failed_start();
    run(&w.cx, &goal_req()).unwrap_err();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, db) = (seen.clone(), w.dir.path().join("moor.db"));
    w.driver.on_exec(move || {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1)));
        let row = Store::open(&db, clock).unwrap().task(TaskId::new(1));
        sink.lock().unwrap().push(row.unwrap().unwrap());
    });
    run(&w.cx, &goal_req()).unwrap();
    let mut rows = seen.lock().unwrap().split_off(0);
    assert_eq!(rows.len(), 1);
    let during = rows.pop().unwrap();
    assert_eq!(during.state, TaskState::SettingUp);
    assert!(matches!(&during.intent, Some(Intent::Setup { pid }) if *pid == std::process::id()));
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Working);
    assert!(row.intent.is_none());
}

#[test]
fn a_failed_retry_clears_the_setup_intent() {
    let src = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(1)), Ok(exit(1))]);
    let (w, _, _repo) = dotfiles_world("setup: [a]\n", Some(src.path()), None, driver);
    run(&w.cx, &goal_req()).unwrap_err();
    run(&w.cx, &goal_req()).unwrap_err();
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    assert_eq!(row.state, TaskState::SetupFailed);
    assert!(row.intent.is_none());
}
