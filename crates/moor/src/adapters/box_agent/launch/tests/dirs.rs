//! The directories `launch_dirs` reports are the ones `launch` creates and removes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq};
use moor_core::seam::surface::Surface;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::fake::ready_release;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let d = Arc::new(
        d.peek_text("box-1", &environ, ENVIRON)
            .run_stdout("Linux x86_64"),
    );
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

fn req(s: &FakeSurface) -> LaunchReq {
    let wt = s.create_worktree(
        &PathBuf::from("/r"),
        &BoxName::parse_allow_bare("t1").unwrap(),
        None,
    );
    let wt = wt.unwrap();
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: wt.path.clone(),
        workspace: s.ensure_workspace(&wt, "moor:t1").unwrap(),
        task: BoxName::parse_allow_bare("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        goal: Some("fix x".to_owned()),
        resume: None,
        issue: None,
        setup_exit: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        origin: None,
        strip_env: Vec::new(),
    }
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        ready_release(),
    )
}

fn unquote(q: &str) -> String {
    let inner = q.strip_prefix('\'').and_then(|q| q.strip_suffix('\''));
    inner.unwrap().replace("'\\''", "'")
}

/// The script the pane line asks the Box to run under `sh -c`.
fn script_of(line: &str) -> String {
    let rest = line.strip_prefix("'ssh' '-t' 'box-1' ").unwrap();
    let quoted = rest
        .strip_suffix(r#"; c=$?; printf '\r\033[K'; echo "moor: agent exited $c""#)
        .unwrap();
    unquote(unquote(quoted).strip_prefix("sh -c ").unwrap())
}

#[test]
fn launch_dirs_names_the_dirs_the_launch_creates() {
    let (d, s) = (driver(FakeDriver::new()), Arc::new(FakeSurface::new("/r")));
    let (r, agent) = (req(&s), agent(&d, &s));
    let (cred, tmp) = agent.launch_dirs(&r).unwrap().unwrap();
    agent.launch(&r).unwrap();
    let brief = d.puts().into_iter().next().unwrap().1;
    assert_eq!(brief.as_str(), format!("{}/brief.md", cred.as_str()));
    let script = script_of(&s.opened().remove(0).argv.pop().unwrap());
    let (cred, tmp) = (cred.as_str(), tmp.as_str());
    for want in [
        format!("trap 'rm -rf '\\''{cred}'\\'' '\\''{tmp}'\\''' EXIT"),
        format!("mkdir -p -m 700 '{tmp}'"),
        format!("export TMPDIR='{tmp}'"),
    ] {
        assert!(script.contains(&want), "{want}: {script}");
    }
}

#[test]
fn launch_dirs_fails_at_the_home_step_when_the_home_cannot_be_read() {
    let d = driver(FakeDriver::new().fail_peek(DriverError::Timeout(Duration::from_secs(1))));
    let s = Arc::new(FakeSurface::new("/r"));
    let err = agent(&d, &s).launch_dirs(&req(&s)).unwrap_err();
    assert!(
        matches!(err, EngineError::Failed { step: "home", .. }),
        "{err:?}"
    );
}
