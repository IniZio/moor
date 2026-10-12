//! Tests for the memoised credential checks.

use std::path::Path;
use std::sync::Arc;

use moor_core::config::{parse_user, Flags, TaskSettings};
use moor_core::seam::proc::Tool;

use super::{checked, unresolved};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

/// Context over `proc`, rooted at `dir`.
fn cx(dir: &Path, proc: &Arc<ScriptedProc>) -> Cx {
    Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        Arc::clone(proc) as Arc<_>,
    )
}

/// Settings resolved from user config `yaml` alone.
fn settings(yaml: &str) -> TaskSettings {
    let user = parse_user("u.yaml", yaml).expect("user config");
    TaskSettings::resolve(&user, None, &Flags::default())
}

/// How many `sh` calls the proc has seen.
fn sh_calls(proc: &ScriptedProc) -> usize {
    proc.calls().iter().filter(|c| c.tool == Tool::Sh).count()
}

const SHARED: &str = "
credentials:
  A: {cmd: [echo, same]}
  B: {cmd: [echo, same]}
mcp_servers:
  m: {url: 'https://x/${A}'}
";

#[test]
fn shared_source_is_checked_once() {
    let d = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Sh, &[], Reply::Ok("v".to_owned()));
    let out = checked(&cx(d.path(), &proc), &settings(SHARED), d.path());
    assert_eq!(sh_calls(&proc), 1);
    assert_eq!(out.credentials.len(), 2);
    assert!(out.credentials.iter().all(|i| i.resolves == Some(true)));
    assert_eq!(out.mcp.len(), 1);
}

#[test]
fn unused_credential_is_not_run() {
    let d = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(Tool::Sh, &[], Reply::Ok("v".to_owned()));
    let yaml = "
credentials:
  A: {cmd: [echo, used]}
  B: {cmd: [echo, unused]}
mcp_servers:
  m: {url: 'https://x/${A}'}
";
    let failing = unresolved(&cx(d.path(), &proc), &settings(yaml), d.path());
    assert!(failing.is_empty());
    let calls = proc.calls();
    assert_eq!(sh_calls(&proc), 1);
    assert!(calls.iter().all(|c| !c.args.iter().any(|a| a == "unused")));
}

#[test]
fn failing_source_is_reported_once_per_row() {
    let d = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Sh,
        &[],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "boom".to_owned(),
        },
    );
    let failing = unresolved(&cx(d.path(), &proc), &settings(SHARED), d.path());
    assert_eq!(sh_calls(&proc), 1);
    let names: Vec<&str> = failing.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["A", "m"]);
    assert!(failing.iter().all(|i| i.error.is_some()));
}
