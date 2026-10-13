use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq};
use moor_core::seam::surface::{Surface, SurfaceError};
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";
const CRED: &str = "/home/box/.moor-cred-t1-abc123";

fn environ_path() -> BoxPath {
    BoxPath::restore("/proc/self/environ").unwrap()
}

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(d.peek_text("box-1", &environ_path(), ENVIRON));
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        egress: EgressPlan::Open,
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

fn surface(s: FakeSurface) -> Arc<FakeSurface> {
    Arc::new(s)
}

fn req(s: &FakeSurface) -> LaunchReq {
    let wt = s.create_worktree(&PathBuf::from("/r"), &TaskName::parse("t1").unwrap(), None);
    let wt = wt.unwrap();
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: wt.path.clone(),
        workspace: s.ensure_workspace(&wt, "moor:t1").unwrap(),
        task: TaskName::parse("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        goal: Some("fix x".to_owned()),
        issue: None,
        setup_exit: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
    }
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    BoxAgent::new(d.clone(), s.clone(), Arc::new(Claude))
}

fn launch(
    d: FakeDriver,
    s: FakeSurface,
) -> (
    Arc<FakeDriver>,
    Arc<FakeSurface>,
    Result<String, EngineError>,
) {
    let (d, s) = (driver(d), surface(s));
    let r = req(&s);
    let out = agent(&d, &s)
        .launch(&r)
        .map(|s| s.pane.unwrap().as_str().to_owned());
    (d, s, out)
}

fn pane_line(s: &FakeSurface) -> String {
    s.opened().remove(0).argv.pop().unwrap()
}

#[test]
fn delivers_the_brief_under_the_box_home_with_the_worktree_as_host_cwd() {
    let (d, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    assert_eq!(out.unwrap(), "p-2");
    let (handle, path, bytes, mode, cwd) = d.puts().into_iter().next().unwrap();
    assert_eq!(handle, BoxHandle::new("box-1"));
    assert_eq!(path.as_str(), format!("{CRED}/brief.md"));
    assert!(String::from_utf8_lossy(&bytes).contains("Goal: fix x"));
    assert_eq!(mode, 0o600);
    assert_eq!(Some(cwd), s.opened().remove(0).cwd);
}

#[test]
fn delivers_the_hook_writer_files() {
    let (d, _, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let paths: Vec<_> = d.puts().iter().map(|p| p.1.as_str().to_owned()).collect();
    assert_eq!(
        paths,
        [
            format!("{CRED}/brief.md"),
            "/work/box-1/.moor/hook.sh".to_owned(),
            "/work/box-1/.moor/hooks.json".to_owned(),
        ]
    );
}

#[test]
fn opens_an_unfocused_agent_tab_in_the_host_worktree() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let cmd = s.opened().remove(0);
    assert_eq!(cmd.label, "agent:t1");
    assert_eq!(cmd.cwd, Some(PathBuf::from("/r/wt/t1")));
    assert!(cmd.env.is_empty());
    assert!(!s.calls().iter().any(|c| c.starts_with("focus")));
}

#[test]
fn the_pane_line_runs_the_script_without_exec_and_reports_the_exit() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let line = pane_line(&s);
    assert!(line.starts_with("'ssh' '-t' 'box-1' '"), "{line}");
    assert!(line.ends_with("; echo \"moor: agent exited $?\""), "{line}");
    assert!(!line.contains("exec "), "{line}");
}

#[test]
fn the_script_cleans_up_installs_the_brief_and_starts_the_agent() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let line = pane_line(&s);
    let script = script_of(&line);
    for want in [
        &format!("trap 'rm -rf '\\''{CRED}'\\''' EXIT"),
        &format!("install -m 600 '{CRED}/brief.md' '/work/box-1/.moor/brief.md'"),
        r#""state":"starting","event":"Launch""#,
        "> '/work/box-1/.moor/agent-state.json'",
        "settings.json",
        ".claude.json",
        "cd '/work/box-1' && 'claude' '--model' 'haiku'",
    ] {
        assert!(script.contains(want), "{want}: {script}");
    }
    assert!(!line.contains("bypassPermissions"));
    assert!(!line.contains("ANTHROPIC"));
}

fn script_of(line: &str) -> String {
    let rest = line.strip_prefix("'ssh' '-t' 'box-1' ").unwrap();
    let quoted = rest
        .strip_suffix("; echo \"moor: agent exited $?\"")
        .unwrap();
    let inner = quoted.strip_prefix('\'').and_then(|q| q.strip_suffix('\''));
    inner.unwrap().replace("'\\''", "'")
}

#[test]
fn the_script_is_valid_shell() {
    let (_, s, out) = launch(FakeDriver::new(), FakeSurface::new("/r"));
    out.unwrap();
    let mut sh = Command::new("sh")
        .arg("-n")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin
        .take()
        .unwrap()
        .write_all(script_of(&pane_line(&s)).as_bytes())
        .unwrap();
    assert!(sh.wait().unwrap().success());
}

#[test]
fn a_failed_connect_lookup_opens_no_pane_and_delivers_nothing() {
    let (d, s, out) = launch(
        FakeDriver::new().fail_connect(DriverError::NotFound),
        FakeSurface::new("/r"),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed {
            step: "connect",
            ..
        })
    ));
    assert!(s.opened().is_empty());
    assert!(d.puts().is_empty());
    assert!(d.execs().is_empty());
}

#[test]
fn a_failed_brief_write_removes_the_cred_dir() {
    let (d, s, out) = launch(
        FakeDriver::new().fail_put(DriverError::Timeout(std::time::Duration::from_secs(30))),
        FakeSurface::new("/r"),
    );
    assert!(matches!(
        out,
        Err(EngineError::Failed { step: "brief", .. })
    ));
    assert!(s.opened().is_empty());
    let execs = d.execs();
    assert_eq!(execs.len(), 1);
    assert_eq!(
        execs.into_iter().next().unwrap().1.argv,
        ["rm", "-rf", CRED]
    );
}

#[test]
fn a_failed_pane_open_removes_the_cred_dir() {
    let (d, _, out) = launch(
        FakeDriver::new(),
        FakeSurface::new("/r").fail_open_pane(SurfaceError::Timeout),
    );
    assert!(matches!(out, Err(EngineError::Failed { step: "pane", .. })));
    assert_eq!(d.execs().remove(0).1.argv, ["rm", "-rf", CRED]);
}

#[test]
fn a_failed_cleanup_is_named_in_the_error() {
    let (_, _, out) = launch(
        FakeDriver::new()
            .fail_put(DriverError::NotFound)
            .exec_results([Ok(moor_core::seam::driver::ExecOutcome::Exited { code: 1 })]),
        FakeSurface::new("/r"),
    );
    let message = match out {
        Err(EngineError::Failed {
            step: "brief",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    };
    assert!(
        message.contains(&format!("could not remove {CRED}")),
        "{message}"
    );
}

#[test]
fn a_box_without_a_home_fails_before_delivering() {
    let d = Arc::new(FakeDriver::new().peek_text("box-1", &environ_path(), "PATH=/usr/bin\0"));
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        egress: EgressPlan::Open,
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    let s = surface(FakeSurface::new("/r"));
    let r = req(&s);
    let out = agent(&d, &s).launch(&r);
    assert!(matches!(out, Err(EngineError::Failed { step: "home", .. })));
    assert!(d.puts().is_empty());
    assert!(s.opened().is_empty());
}
