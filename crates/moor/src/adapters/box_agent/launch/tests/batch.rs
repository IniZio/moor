//! A launch costs three Box calls after the connect lookup: the home lookup, the files, the start.

use std::sync::Arc;

use moor_core::seam::agent_adapter::ProxyAddr;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, LaunchPrompt, LaunchReq};
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::driver::FakeDriver;

const ENVIRON: &str = "PATH=/usr/bin\0HOME=/home/box\0LANG=C\0";

fn exited(codes: &[i32]) -> Vec<Result<ExecOutcome, DriverError>> {
    codes
        .iter()
        .map(|&code| Ok(ExecOutcome::Exited { code }))
        .collect()
}

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

fn req(proxy: bool) -> LaunchReq {
    let name = BoxName::parse_allow_bare("t1").unwrap();
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(1),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        box_name: name,
        branch: "moor/t1".to_owned(),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        disk_note: None,
        shape: None,
        model: Some("haiku".to_owned()),
        nonce: "abc123".to_owned(),
        proxy: proxy.then(|| {
            Arc::new(ProxyAddr {
                box_port: 4567,
                ca_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
                box_env: Vec::new(),
                egress_port: 4568,
                hosts: Vec::new(),
            })
        }),
        strip_env: Vec::new(),
        prompt: LaunchPrompt::None,
    }
}

fn agent(d: &Arc<FakeDriver>) -> BoxAgent {
    BoxAgent::new(d.clone(), Arc::new(Claude))
}

struct Run {
    d: Arc<FakeDriver>,
    out: Result<(), EngineError>,
}

fn launch(proxy: bool, check_codes: &[i32]) -> Run {
    let d = driver(FakeDriver::new().exec_results(exited(check_codes)));
    let out = agent(&d).launch(&req(proxy)).map(|_| ());
    Run { d, out }
}

fn box_calls(d: &FakeDriver) -> Vec<String> {
    let mut calls = d.box_calls();
    assert_eq!(calls.remove(0), "acquire");
    calls
}

fn assert_connect_argv_reused(d: &FakeDriver) {
    let connects = d.batch_connects();
    assert!(connects.iter().all(|c| !c.is_empty()), "{connects:?}");
    assert!(connects.iter().all(|c| Some(c) == connects.first()));
}

#[test]
fn a_warm_proxy_launch_makes_three_batch_calls_after_the_connect_lookup() {
    let run = launch(true, &[0, 0]);
    run.out.unwrap();
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "batch"]
    );
    assert_eq!(run.d.batch_connects().len(), 3);
    assert_connect_argv_reused(&run.d);
}

#[test]
fn a_launch_without_a_proxy_makes_three_batch_calls_after_the_connect_lookup() {
    let run = launch(false, &[]);
    run.out.unwrap();
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "batch"]
    );
    assert_connect_argv_reused(&run.d);
}

fn agent_failure(run: Run) -> String {
    match run.out {
        Err(EngineError::Failed {
            step: "agent",
            message,
        }) => message,
        other => format!("unexpected {other:?}"),
    }
}

#[test]
fn a_missing_agent_refuses_a_proxy_launch_before_anything_is_uploaded() {
    let run = launch(true, &[1]);
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "exec"]
    );
    assert!(run.d.puts().is_empty());
    let message = agent_failure(run);
    assert!(message.contains("`claude`"), "{message}");
    assert!(message.contains("`setup`"), "{message}");
}

#[test]
fn a_missing_agent_refuses_a_launch_without_a_proxy_before_anything_is_uploaded() {
    let run = launch(false, &[127]);
    assert_eq!(
        box_calls(&run.d),
        ["connect_argv", "batch", "batch", "exec"]
    );
    assert!(run.d.puts().is_empty());
    assert!(agent_failure(run).contains("not on the Box's PATH"));
}

#[test]
fn the_agent_check_names_the_program_and_the_launch_path() {
    let run = launch(false, &[]);
    run.out.unwrap();
    let execs = run.d.execs();
    let script = execs
        .iter()
        .filter_map(|e| e.1.argv.get(2))
        .find(|a| a.contains("command -v"))
        .expect("agent check");
    assert_eq!(
        script,
        "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"; command -v 'claude' >/dev/null 2>&1"
    );
}

#[test]
fn a_launch_with_no_environ_reports_the_home_step() {
    let d = Arc::new(FakeDriver::new());
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
    let out = agent(&d).launch(&req(true));
    let got = match out {
        Err(EngineError::Failed { step, message }) => format!("{step}: {message}"),
        other => format!("unexpected {other:?}"),
    };
    assert_eq!(got, "home: the Box has no process environment");
}
