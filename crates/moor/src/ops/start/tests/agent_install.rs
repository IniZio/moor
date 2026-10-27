use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::{AgentConfig, DotfilesConfig, UserConfig};
use moor_core::seam::agent_profile::AgentInstall;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{BoxId, StartReq};

use super::setup::unwrapped;
use super::state::WORKING;
use super::{repo_req, script_base_yaml, world_with, World};
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
fn a_failed_install_warns_with_the_log_tail_and_the_start_goes_on() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0)), Ok(exit(1))]);
    let (w, engine) = install_world("setup: [a]\n", None, true, driver);
    log_writer(&w);
    let v = run(&w.cx, &prompt_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ok);
    assert_eq!(engine.launches().len(), 1);
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
    assert!(events.iter().all(|e| e.kind != "setup_failed"));
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
    assert_eq!(engine.launches().len(), 1);
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
