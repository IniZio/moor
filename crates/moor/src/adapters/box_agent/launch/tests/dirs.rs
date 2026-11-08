//! The directories `launch_dirs` reports are the ones `launch` creates and removes.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::engine::{Engine, EngineError, LaunchPrompt, LaunchReq};
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::driver::FakeDriver;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";

fn driver(d: FakeDriver) -> Arc<FakeDriver> {
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let d = Arc::new(d.peek_text("box-1", &environ, ENVIRON));
    d.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    d
}

fn req() -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(1),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        box_name: BoxName::parse_allow_bare("t1").unwrap(),
        branch: "moor/t1".to_owned(),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        disk_note: None,
        shape: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: None,
        strip_env: Vec::new(),
        prompt: LaunchPrompt::None,
    }
}

fn agent(d: &Arc<FakeDriver>) -> BoxAgent {
    BoxAgent::new(d.clone(), Arc::new(Claude))
}

/// The agent script the launch put on the Box.
fn script_of(d: &FakeDriver) -> String {
    let put = d
        .puts()
        .into_iter()
        .find(|p| p.1.as_str().ends_with("/agent.sh"));
    String::from_utf8(put.expect("the launch puts the agent script").2).unwrap()
}

#[test]
fn launch_dirs_names_the_dirs_the_launch_creates() {
    let d = driver(FakeDriver::new());
    let (r, agent) = (req(), agent(&d));
    let (cred, tmp) = agent.launch_dirs(&r).unwrap().unwrap();
    agent.launch(&r).unwrap();
    let brief = d.puts().into_iter().next().unwrap().1;
    assert_eq!(brief.as_str(), format!("{}/brief.md", cred.as_str()));
    let script = script_of(&d);
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
    let err = agent(&d).launch_dirs(&req()).unwrap_err();
    assert!(
        matches!(err, EngineError::Failed { step: "home", .. }),
        "{err:?}"
    );
}
