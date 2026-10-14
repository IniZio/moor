//! Tests for the detached daemon spawn using a shell script as the program.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, ProcSpec, Tool};
use rustix::process::{getpgid, Pid};
use tempfile::TempDir;

use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;

fn script(dir: &Path, body: &str) -> String {
    let path = dir.join("git");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn daemon_logs_stderr_only_in_own_group() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "echo to-out; echo to-err >&2; sleep 1");
    let env = Env::from_pairs([("MOOR_GIT_BIN".to_owned(), bin)]);
    let p = SystemProc::new(&env, &ToolPaths::default());
    let log = dir.path().join("state").join("proxy").join("t1.log");
    let spec = ProcSpec::new(Tool::Git, Deadline(Duration::from_secs(5)));

    let pid = p.daemon(&spec, &log).unwrap();
    assert!(pid > 0);

    let mut text = String::new();
    for _ in 0..100 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains("to-err") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(text.contains("to-err"), "log: {text:?}");
    assert!(!text.contains("to-out"), "log: {text:?}");
    assert_eq!(mode(&log), 0o600);
    assert_eq!(mode(log.parent().unwrap()), 0o700);

    let child = Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    let group = getpgid(Some(child)).unwrap();
    assert_eq!(group, child, "child leads its own group");
    assert_ne!(group, getpgid(None).unwrap());
}

#[test]
fn moor_self_daemon_inherits_environment() {
    let dir = TempDir::new().unwrap();
    let bin = script(dir.path(), "echo \"manifest=$CARGO_MANIFEST_DIR\" >&2");
    let env = Env::from_pairs([("MOOR_SELF_BIN".to_owned(), bin)]);
    let p = SystemProc::new(&env, &ToolPaths::default());
    let log = dir.path().join("self.log");
    let spec = ProcSpec::new(Tool::MoorSelf, Deadline(Duration::from_secs(5)));
    let want = format!("manifest={}", std::env::var("CARGO_MANIFEST_DIR").unwrap());

    p.daemon(&spec, &log).unwrap();

    let mut text = String::new();
    for _ in 0..100 {
        text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains(&want) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(text.contains(&want), "log: {text:?}");
}
