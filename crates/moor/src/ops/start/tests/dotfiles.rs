use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::config::{DotfilesConfig, UserConfig};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::view::{DotfilesView, StartView};
use moor_core::{BoxId, StartReq};

use super::setup::{
    scripted, unwrapped,
    Step::{Cmd, Sync},
};
use super::state::WORKING;
use super::{req, world_with, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::Reply;
use crate::testing::proxy_host::FakeProxyHost;

const TAR_SCRIPT: &str = "exec tar -C \"$1\" -czhf - .";

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

fn goal_req() -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    }
}

/// The Box script text for the upload at `archive` with install line `install`.
fn script(archive: &str, install: &str) -> String {
    let qa = quote(archive);
    let qs = quote(archive.strip_suffix("/dotfiles.tgz").unwrap());
    format!(
        "set -e; trap \"rm -rf {qs}\" EXIT; rm -rf \"$HOME/.moor-dotfiles.moor-tmp\"; \
         mkdir -p \"$HOME/.moor-dotfiles.moor-tmp\"; \
         tar -xzf {qa} -C \"$HOME/.moor-dotfiles.moor-tmp\"; \
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
        ..crate::testing::cx::fake_config()
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
        .filter(|e| {
            let mkdir = e.1.argv.first().map(String::as_str) == Some("mkdir");
            e.1.sync == SyncMode::None && e.1.cwd.is_none() && !mkdir
        })
        .map(|e| (unwrapped(e.1.argv), e.2, e.1.sync))
        .collect()
}

/// Whether an upload is the exec-env staging (nonce, CA), not a dotfiles archive.
fn is_staged(path: &str) -> bool {
    path.contains("/.moor-cred-")
}

fn started(w: &World) -> StartView {
    run(&w.cx, &goal_req()).unwrap()
}

/// The one upload: the scripted tar bytes in a per-run random directory, owner-only.
/// Returns the archive path.
fn assert_archive_put(w: &World) -> String {
    let puts: Vec<_> = (w.driver.puts().into_iter())
        .filter(|p| !is_staged(p.1.as_str()))
        .collect();
    assert_eq!(puts.len(), 1);
    let (handle, path, bytes, mode, _) = puts.first().unwrap();
    assert_eq!(handle.as_str(), "box-1");
    let hex = path
        .as_str()
        .strip_prefix("/tmp/moor-dotfiles-1-")
        .and_then(|p| p.strip_suffix("/dotfiles.tgz"))
        .unwrap();
    assert!(
        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "{hex}"
    );
    assert_eq!(bytes, &[0x1f, 0x8b, 1, 2, 3]);
    assert_eq!(*mode, 0o600);
    path.as_str().to_owned()
}

/// The last exec (after the checkout sync and setup) is the dotfiles script.
fn assert_dotfiles_exec(w: &World, archive: &str) {
    let want = ["sh", "-c", &script(archive, DEFAULT_INSTALL)].map(str::to_owned);
    let execs = w.driver.execs();
    assert_eq!(execs.len(), 5);
    let (_, spec, deadline) = execs.last().unwrap();
    assert!(spec
        .argv
        .get(2)
        .is_some_and(|p| p.contains("export HTTPS_PROXY=")));
    assert_eq!(unwrapped(spec.argv.clone()), want);
    assert_eq!(spec.sync, SyncMode::None);
    assert_eq!(spec.cwd, None);
    assert!(spec.env.contains(&(
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        "moor-proxy".to_owned()
    )));
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
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.agent.launched);
    assert_eq!(engine.launches().len(), 1);

    let archive = assert_archive_put(&w);
    assert_dotfiles_exec(&w, &archive);
    assert_tar_call(&w, src.path());

    let calls = w.driver.calls();
    assert_eq!(
        calls,
        [
            "acquire",
            "exec box-1",
            "connect_argv box-1",
            "peek_file box-1",
            "exec box-1",
            "connect_argv box-1",
            "put_file box-1",
            "put_file box-1",
            "exec box-1",
            "copy_out box-1",
            "put_file box-1",
            "connect_argv box-1",
            "peek_file box-1",
            "exec box-1",
            "connect_argv box-1",
            "put_file box-1",
            "put_file box-1",
            "exec box-1",
            "sweep box-1"
        ]
    );
    assert!(v.dotfiles.ran && v.dotfiles.ok);
    assert_eq!(v.dotfiles.ms, Some(0));
}

#[test]
fn a_configured_install_script_with_a_space_is_quoted_and_guarded() {
    let src = tempfile::tempdir().unwrap();
    let (w, _, _repo) = dotfiles_world(
        "env: {A: a}\n",
        Some(src.path()),
        Some("my setup.sh"),
        FakeDriver::new(),
    );
    started(&w);
    let archive = w.driver.puts().first().unwrap().1.as_str().to_owned();
    let install = "[ -f './my setup.sh' ] || { echo 'dotfiles install script my setup.sh not found' >&2; exit 1; }; MOOR_DOTFILES=1 bash './my setup.sh'";
    let want = ["sh", "-c", &script(&archive, install)].map(str::to_owned);
    let argvs: Vec<_> = dotfiles_execs(&w).into_iter().map(|e| e.0).collect();
    assert_eq!(argvs, [want]);
}

/// Writes a 25-line host log for the dotfiles exec of Box 1 on each exec.
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
    let driver = FakeDriver::new().exec_results(scripted([
        Sync(Ok(exit(0))),
        Cmd(Ok(exit(0))),
        Cmd(Ok(exit(1))),
    ]));
    let (w, engine, repo) = dotfiles_world("setup: [a]\n", Some(src.path()), None, driver);
    log_writer(&w);
    (w, engine, repo, src)
}

/// The dotfiles warning carries the last 20 log lines.
fn assert_warned_with_tail(w: &World) {
    let warnings = w.cx.take_warnings();
    let warn = warnings
        .iter()
        .find(|x| x.code == "dotfiles_failed")
        .unwrap();
    assert!(warn.message.starts_with("dotfiles failed:\n"), "{warn:?}");
    assert!(warn.message.contains("row-25"), "{warn:?}");
    assert!(warn.message.contains("row-06"), "{warn:?}");
    assert!(!warn.message.contains("row-05"), "{warn:?}");
}

#[test]
fn a_failing_install_does_not_stop_start_and_warns_with_the_log_tail() {
    let (w, engine, _repo, _src) = failed_start();
    let v = run(&w.cx, &goal_req()).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.dotfiles.ran && !v.dotfiles.ok);
    assert_eq!(engine.launches().len(), 1);

    assert_warned_with_tail(&w);

    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.boxed.map(|b| b.state), Some(BoxState::Ready));
    assert!(row.intent.is_none());
    let events = w.cx.store().unwrap().events(BoxId::new(1), 50).unwrap();
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let line = failed.payload.get("line").and_then(|l| l.as_str()).unwrap();
    assert!(
        line.contains("row-25") && !line.contains("row-05"),
        "{line}"
    );
}

#[test]
fn without_dotfiles_config_nothing_is_packed_uploaded_or_run() {
    let (w, _, _repo) = dotfiles_world("setup: [a]\n", None, None, FakeDriver::new());
    let v = started(&w);
    assert_eq!(v.dotfiles, DotfilesView::default());
    assert!(w.proc.calls().iter().all(|c| c.tool != Tool::Sh));
    assert!(w.driver.puts().iter().all(|p| is_staged(p.1.as_str())));
    assert!(dotfiles_execs(&w).is_empty());
}

#[test]
fn dotfiles_still_run_when_a_setup_line_fails() {
    let src = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results(scripted([
        Sync(Ok(exit(0))),
        Cmd(Ok(exit(1))),
        Cmd(Ok(exit(0))),
    ]));
    let (w, engine, _repo) = dotfiles_world("setup: [a]\n", Some(src.path()), None, driver);
    let v = started(&w);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert!(v.setup.ran && !v.setup.ok);
    assert!(v.dotfiles.ran && v.dotfiles.ok);
    assert_eq!(w.driver.execs().len(), 5);
    assert_eq!(dotfiles_execs(&w).len(), 1);
    let (_, spec, _) = w.driver.execs().pop().unwrap();
    assert_eq!(spec.sync, SyncMode::None);
    assert_eq!(engine.launches().len(), 1);
}

#[test]
fn the_host_log_and_its_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (w, _, _repo, _src) = failed_start();
    run(&w.cx, &goal_req()).unwrap();
    let dir = w.dir.path().join("dotfiles");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("1.log")), 0o600);
}

#[test]
fn a_failed_setup_line_still_fails_setup_when_the_dotfiles_fail_too() {
    let src = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new().exec_results(scripted([
        Sync(Ok(exit(0))),
        Cmd(Ok(exit(1))),
        Cmd(Ok(exit(1))),
    ]));
    let (w, engine, _repo) = dotfiles_world("setup: [make-it]\n", Some(src.path()), None, driver);
    log_writer(&w);
    let v = run(&w.cx, &goal_req()).unwrap();
    assert!(v.setup.ran && !v.setup.ok);
    assert!(v.dotfiles.ran && !v.dotfiles.ok);
    assert_eq!(engine.launches().len(), 1);
    let warnings = w.cx.take_warnings();
    assert!(warnings.iter().any(|x| x.code == "dotfiles_failed"));
}

#[test]
fn dotfiles_install_gets_the_repo_env_and_the_proxy_without_any_setup_lines() {
    let src = tempfile::tempdir().unwrap();
    let (w, _, _repo) =
        dotfiles_world("env: {ZED: z}\n", Some(src.path()), None, FakeDriver::new());
    let v = started(&w);
    assert!(v.dotfiles.ran && v.dotfiles.ok);
    let execs = w.driver.execs();
    let (_, spec, _) = execs.last().unwrap();
    assert!(spec
        .argv
        .get(2)
        .is_some_and(|p| p.contains("export HTTPS_PROXY=")));
    assert!(spec.env.contains(&("ZED".to_owned(), "z".to_owned())));
    assert!(spec.env.contains(&(
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        "moor-proxy".to_owned()
    )));
    assert_eq!(spec.cwd, None);
}

#[test]
fn dotfiles_fail_when_the_proxy_cannot_start() {
    let src = tempfile::tempdir().unwrap();
    let (mut w, engine, _repo) =
        dotfiles_world("env: {A: a}\n", Some(src.path()), None, FakeDriver::new());
    w.cx = w.cx.with_proxy_host(Arc::new(
        FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed("boom".into())),
    ));
    let v = run(&w.cx, &goal_req()).unwrap();
    assert!(v.dotfiles.ran && !v.dotfiles.ok);
    let warnings = w.cx.take_warnings();
    let msg = &warnings
        .iter()
        .find(|x| x.code == "dotfiles_failed")
        .unwrap()
        .message;
    assert!(msg.contains("the Proxy could not start: boom"), "{msg}");
    assert!(dotfiles_execs(&w).is_empty());
    assert!(w.proc.calls().iter().all(|c| c.tool != Tool::Sh));
    assert!(engine.launches().is_empty());
}
