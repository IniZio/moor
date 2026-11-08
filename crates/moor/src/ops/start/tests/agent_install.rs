use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use crate::testing::agent;
use moor_core::activity::Probe;
use moor_core::config::{AgentConfig, DotfilesConfig, UserConfig};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::agent_adapter::AgentInstall;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::engine::EngineError;
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{BoxId, StartReq};

use super::setup::{argvs, script_argv, unwrapped};
use super::state::WORKING;
use super::{repo_req, script_base_yaml, world_with, World};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;
use crate::testing::proxy_host::FakeProxyHost;

const SCRIPT: &str = "echo install-the-agent";

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

fn prompt_req() -> StartReq {
    StartReq {
        prompt: Some("ship it".into()),
        ..repo_req("demo")
    }
}

fn installer() -> AgentInstall {
    AgentInstall {
        script: SCRIPT.to_owned(),
        present: "true".to_owned(),
        dirs: vec!["~/.local/share/agent".to_owned()],
        check: "true".to_owned(),
    }
}

/// A world over a repo with `yaml` whose engine installs its agent with [`SCRIPT`] when
/// `install`.
fn install_world(
    yaml: &str,
    user: Option<bool>,
    install: bool,
    driver: FakeDriver,
) -> (World, Arc<FakeEngine>) {
    let config = UserConfig {
        agent: AgentConfig {
            install: user,
            ..AgentConfig::default()
        },
        ..UserConfig::default()
    };
    let mut w = world_with(driver, config);
    script_base_yaml(&w, Some(yaml));
    let engine = FakeEngine::new();
    let engine = Arc::new(if install {
        engine.install(installer())
    } else {
        engine
    });
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

/// Execs that were the install step, with their deadlines.
fn installs(w: &World) -> Vec<(ExecSpec, Deadline)> {
    w.driver
        .execs()
        .into_iter()
        .filter(|e| unwrapped(e.1.argv.clone()) == ["sh", "-c", SCRIPT])
        .map(|e| (e.1, e.2))
        .collect()
}

#[test]
fn the_install_runs_after_setup_and_dotfiles_with_the_proxy_and_a_deadline() {
    let (w, engine) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(engine.launches().len(), 1);
    let execs = w.driver.execs();
    assert_eq!(execs.len(), 3, "clone, setup, install");
    let (_, spec, deadline) = execs.last().unwrap();
    assert_eq!(unwrapped(spec.argv.clone()), ["sh", "-c", SCRIPT]);
    assert!(spec
        .argv
        .get(2)
        .is_some_and(|p| p.contains("export HTTPS_PROXY=")));
    assert_eq!(spec.cwd, None);
    assert_eq!(*deadline, Deadline(Duration::from_mins(10)));
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .all(|x| x.code != "agent_install_failed"));
}

#[test]
fn the_install_runs_after_the_dotfiles_so_a_user_who_installs_their_own_agent_wins() {
    let src = tempfile::tempdir().unwrap();
    let config = UserConfig {
        dotfiles: Some(DotfilesConfig {
            path: src.path().to_path_buf(),
            install: None,
        }),
        ..UserConfig::default()
    };
    let mut w = world_with(FakeDriver::new(), config);
    script_base_yaml(&w, Some("setup: [a]\n"));
    w.proc.on(
        Tool::Sh,
        &["-c", "exec tar -C \"$1\" -czhf - ."],
        Reply::Bytes(vec![0x1f, 0x8b]),
    );
    let engine = Arc::new(FakeEngine::new().install(installer()));
    w.cx = w.cx.with_engine(engine);
    run(&w.cx, &prompt_req()).unwrap();
    let scripts: Vec<String> = w
        .driver
        .execs()
        .into_iter()
        .map(|e| unwrapped(e.1.argv).pop().unwrap())
        .collect();
    assert_eq!(scripts.len(), 4, "clone, setup, dotfiles, install");
    assert!(scripts.get(2).is_some_and(|s| s.contains(".moor-dotfiles")));
    assert_eq!(scripts.last().map(String::as_str), Some(SCRIPT));
}

#[test]
fn an_engine_without_an_installer_runs_nothing_extra() {
    let (w, _) = install_world("setup: [a]\n", None, false, FakeDriver::new());
    run(&w.cx, &prompt_req()).unwrap();
    assert_eq!(w.driver.execs().len(), 2);
}

#[test]
fn the_user_config_switch_turns_the_install_off() {
    for (user, runs) in [(None, 1), (Some(true), 1), (Some(false), 0)] {
        let (w, _) = install_world("setup: [a]\n", user, true, FakeDriver::new());
        run(&w.cx, &prompt_req()).unwrap();
        assert_eq!(installs(&w).len(), runs, "user {user:?}");
    }
}

/// Writes a 25-line host log for the install exec of Box 1 on each exec.
fn log_writer(w: &World) {
    let log = w.dir.path().join("agent-install/1.log");
    w.driver.on_exec(move || {
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let text = (1..=25).fold(String::new(), |mut t, n| {
            writeln!(t, "row-{n:02}").unwrap();
            t
        });
        std::fs::write(&log, text).unwrap();
    });
}

#[test]
fn a_failed_install_warns_with_the_log_tail_and_holds_the_box_in_setup_failed() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, engine) = install_world("setup: [a]\n", None, true, driver);
    log_writer(&w);
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(v.failed_step.as_deref(), Some("agent_install"));
    assert!(v.setup.ok);
    assert!(engine.launches().is_empty());
    let warnings = w.cx.take_warnings();
    let warn = warnings
        .iter()
        .find(|x| x.code == "agent_install_failed")
        .unwrap();
    assert!(
        warn.message
            .starts_with("installing the agent in the Box failed: exit 1\n"),
        "{warn:?}"
    );
    assert!(warn.message.contains("row-25") && warn.message.contains("row-06"));
    assert!(!warn.message.contains("row-05"), "{warn:?}");
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let line = failed.payload.get("line").and_then(|l| l.as_str()).unwrap();
    assert!(line.starts_with("agent install: exit 1\n") && line.contains("row-25"));
}

#[test]
fn an_install_that_cannot_run_warns_with_the_driver_error() {
    let driver = FakeDriver::new().exec_results([
        Ok(exit(0)),
        Ok(exit(0)),
        Err(DriverError::Unavailable("ssh down".into())),
    ]);
    let (w, engine) = install_world("setup: [a]\n", None, true, driver);
    run(&w.cx, &prompt_req()).unwrap();
    assert!(engine.launches().is_empty());
    let warnings = w.cx.take_warnings();
    let warn = warnings
        .iter()
        .find(|x| x.code == "agent_install_failed")
        .unwrap();
    assert!(warn.message.contains("ssh down"), "{warn:?}");
}

#[test]
fn no_install_runs_when_the_proxy_cannot_start() {
    let (mut w, _) = install_world("env: {A: a}\n", None, true, FakeDriver::new());
    w.cx = w.cx.with_proxy_host(Arc::new(
        FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into())),
    ));
    run(&w.cx, &prompt_req()).unwrap_err();
    assert!(installs(&w).is_empty());
}

#[test]
fn the_host_log_and_its_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (w, _) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    run(&w.cx, &prompt_req()).unwrap();
    let dir = w.dir.path().join("agent-install");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("1.log")), 0o600);
}

/// The cache volume names the start handed the Driver.
fn planned(w: &World) -> Vec<String> {
    let plans = w.driver.acquired_plans();
    plans
        .iter()
        .flat_map(|p| p.volumes().iter().map(|v| v.name.as_str().to_owned()))
        .collect()
}

#[test]
fn the_install_dir_is_kept_as_a_cache_volume_unless_the_install_is_off() {
    for (user, install, kept) in [
        (None, true, true),
        (Some(false), true, false),
        (None, false, false),
    ] {
        let driver = FakeDriver::new().cache_volumes();
        let (w, _) = install_world("setup: [a]\n", user, install, driver);
        run(&w.cx, &prompt_req()).unwrap();
        assert_eq!(
            planned(&w).contains(&"agent".to_owned()),
            kept,
            "{user:?} {install}"
        );
    }
}

#[test]
fn a_provider_without_cache_volumes_warns_nothing_about_the_install_dir() {
    let (w, _) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    run(&w.cx, &prompt_req()).unwrap();
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .all(|x| !x.code.starts_with("cache_")));
}

/// Steps that run between the machine and the agent, which have their own tests.
pub(super) const WHOLE_START_STEPS: [&str; 4] = ["acquire", "proxy", "clone_others", "launch"];

/// The post-clone step events of Box 1, oldest first, as `kind` and payload.
pub(super) fn step_events(w: &World, id: BoxId) -> Vec<(String, serde_json::Value)> {
    step_events_where(w, id, |step| !WHOLE_START_STEPS.contains(&step))
}

/// The step events of Box 1 whose step passes `keep`, oldest first, as `kind` and payload.
pub(super) fn step_events_where(
    w: &World,
    id: BoxId,
    keep: impl Fn(&str) -> bool,
) -> Vec<(String, serde_json::Value)> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind.starts_with("step_"))
        .filter(|e| {
            keep(
                e.payload
                    .get("step")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default(),
            )
        })
        .map(|e| (e.kind, e.payload))
        .collect()
}

#[test]
fn the_install_records_when_it_starts_and_how_it_ended() {
    let (w, _) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    run(&w.cx, &prompt_req()).unwrap();
    assert_eq!(
        step_events(&w, BoxId::new(1)),
        [
            (
                "step_started".to_owned(),
                serde_json::json!({"step": "agent_install"})
            ),
            (
                "step_finished".to_owned(),
                serde_json::json!({"step": "agent_install", "ms": 0, "ok": true})
            ),
        ]
    );
}

#[test]
fn a_failed_install_records_ok_false() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(2))]);
    let (w, _) = install_world("setup: [a]\n", None, true, driver);
    run(&w.cx, &prompt_req()).unwrap();
    let last = step_events(&w, BoxId::new(1)).pop().unwrap();
    assert_eq!(
        last.1,
        serde_json::json!({"step": "agent_install", "ms": 0, "ok": false})
    );
}

#[test]
fn an_install_that_hits_its_deadline_warns_with_the_step_and_the_limit() {
    let driver = FakeDriver::new().exec_results([
        Ok(exit(0)),
        Ok(exit(0)),
        Err(DriverError::Timeout(Duration::from_mins(10))),
    ]);
    let (w, _) = install_world("setup: [a]\n", None, true, driver);
    run(&w.cx, &prompt_req()).unwrap();
    let warnings = w.cx.take_warnings();
    let warn = warnings
        .iter()
        .find(|x| x.code == "agent_install_failed")
        .unwrap();
    assert!(
        warn.message.contains("agent install ran past 10m"),
        "{warn:?}"
    );
}

#[test]
fn an_install_that_does_not_run_records_no_step() {
    let (w, _) = install_world("setup: [a]\n", Some(false), true, FakeDriver::new());
    run(&w.cx, &prompt_req()).unwrap();
    assert!(step_events(&w, BoxId::new(1)).is_empty());
}

#[test]
fn each_repo_setup_line_is_recorded_as_it_starts_and_ends() {
    let driver = FakeDriver::new().exec_stdout(["", "moor setup: $ a\nmoor setup: $ b\n"]);
    let (w, _) = install_world("setup: [a, b]\n", None, false, driver);
    run(&w.cx, &prompt_req()).unwrap();
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let lines: Vec<_> = (events.into_iter())
        .filter(|e| e.kind.starts_with("setup_line_"))
        .map(|e| (e.kind, e.payload))
        .collect();
    assert_eq!(
        lines,
        [
            (
                "setup_line_started".to_owned(),
                serde_json::json!({"index": 1, "total": 2, "line": "a"})
            ),
            (
                "setup_line_finished".to_owned(),
                serde_json::json!({"index": 1, "total": 2, "ms": 0, "ok": true})
            ),
            (
                "setup_line_started".to_owned(),
                serde_json::json!({"index": 2, "total": 2, "line": "b"})
            ),
            (
                "setup_line_finished".to_owned(),
                serde_json::json!({"index": 2, "total": 2, "ms": 0, "ok": true})
            ),
        ]
    );
}

/// A world whose first start died in the agent install after its setup line passed, then restarted.
fn install_cut_off() -> (World, Arc<FakeEngine>) {
    let (w, engine) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    let execs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    w.driver.on_exec(move || {
        if execs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
            std::panic::resume_unwind(Box::new("process died mid-install"));
        }
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &prompt_req()));
    }));
    assert!(crashed.is_err());
    w.driver.on_exec(|| {});
    assert_eq!(w.driver.execs().len(), 2, "clone, setup");
    fail_interrupted(&w.cx, w.cx.store().unwrap()).unwrap();
    (w, engine)
}

/// A world whose start died on its `nth` exec into the Box, then restarted.
fn start_died_on_exec(nth: usize) -> (World, Arc<FakeEngine>) {
    let (w, engine) = install_world("setup: [a]\n", None, true, FakeDriver::new());
    let execs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    w.driver.on_exec(move || {
        if execs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == nth {
            std::panic::resume_unwind(Box::new("process died mid-setup"));
        }
    });
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, &prompt_req()));
    }));
    assert!(crashed.is_err());
    w.driver.on_exec(|| {});
    (w, engine)
}

#[test]
fn a_setup_followed_after_a_restart_launches_the_agent_with_its_prompt() {
    let (w, engine) = start_died_on_exec(1);
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    crate::ops::common::provision::resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    assert_eq!(engine.launches().len(), 1);
}

/// A world whose start died while the engine launched its agent.
fn launch_cut(req: &StartReq) -> (World, Arc<FakeEngine>) {
    let (w, engine) = install_world("setup: [a]\n", None, false, FakeDriver::new());
    engine.push_launch(Err(EngineError::Failed {
        step: "cut",
        message: String::new(),
    }));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(run(&w.cx, req));
    }));
    assert!(crashed.is_err());
    assert_eq!(engine.launches().len(), 1);
    (w, engine)
}

#[test]
fn a_launch_already_in_flight_is_adopted_and_never_launched_twice() {
    let (w, engine) = launch_cut(&prompt_req());
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    crate::ops::common::provision::resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    assert_eq!(engine.launches().len(), 1, "no second agent");
    assert!(store.live_session(BoxId::new(1)).unwrap().is_some());
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
}

#[test]
fn a_launch_that_never_reached_the_box_is_launched_with_its_prompt() {
    let (w, engine) = launch_cut(&prompt_req());
    engine.set_probes("moor-1", agent::gone());
    let store = w.cx.store().unwrap();
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
    crate::ops::common::provision::resume_setup(&w.cx, store, BoxId::new(1)).unwrap();
    assert_eq!(engine.launches().len(), 2);
    assert!(store.live_session(BoxId::new(1)).unwrap().is_some());
}

#[test]
fn a_box_that_cannot_say_whether_its_agent_runs_launches_nothing() {
    let (w, engine) = launch_cut(&prompt_req());
    engine.set_probes("moor-1", Probe::TimedOut);
    let store = w.cx.store().unwrap();
    fail_interrupted(&w.cx, store).unwrap();
    assert!(crate::ops::common::provision::resume_setup(&w.cx, store, BoxId::new(1)).is_err());
    assert_eq!(engine.launches().len(), 1);
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [BoxId::new(1)]);
}

#[test]
fn a_box_started_with_no_agent_gets_none_after_a_restart() {
    let (w, engine) = install_world("setup: [a]\n", None, false, FakeDriver::new());
    let req = StartReq {
        no_agent: true,
        ..prompt_req()
    };
    run(&w.cx, &req).unwrap();
    assert!(fail_interrupted(&w.cx, w.cx.store().unwrap())
        .unwrap()
        .is_empty());
    assert!(engine.launches().is_empty());
}

#[test]
fn a_restart_during_the_install_is_not_a_failed_setup() {
    let (w, engine) = install_cut_off();
    assert!(engine.launches().is_empty());
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.intent, None);
    assert_eq!(row.setup_failed_at, None);
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    assert!(
        events.iter().all(|e| e.kind != "setup_failed"),
        "{events:?}"
    );
}

#[test]
fn a_start_after_a_restart_during_the_install_installs_then_launches() {
    let (w, engine) = install_cut_off();
    drop(w.cx.take_warnings());
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(installs(&w).len(), 1);
    assert_eq!(engine.launches().len(), 1);
    let line = script_argv(&["a"]);
    let lines = argvs(&w).into_iter().filter(|a| *a == line).count();
    assert_eq!(lines, 1, "the setup line is not run again");
    let codes: Vec<_> = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    assert!(!codes.iter().any(|c| c == "setup_not_rerun"), "{codes:?}");
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.setup_failed_at, None);
}

#[test]
fn a_failed_install_after_a_restart_holds_the_box_in_setup_failed() {
    let (w, engine) = install_cut_off();
    w.driver.push_exec_result(Ok(exit(1)));
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(installs(&w).len(), 1);
    assert!(engine.launches().is_empty());
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let line = failed.payload.get("line").and_then(|l| l.as_str()).unwrap();
    assert!(line.starts_with("agent install: exit 1"), "{line}");
}
