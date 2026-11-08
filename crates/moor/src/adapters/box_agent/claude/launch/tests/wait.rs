//! The hook marks work the Box can see, and the wait check looks for that work's processes.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::super::{wait, Claude};
use super::{ctx, forced_with_core, install, note_of, on_host, run_hook_on, Interp};
use crate::testing::hc;
use moor_core::seam::agent_adapter::AgentAdapter;

fn check(proc: &Path) -> String {
    let script = wait::script(proc.to_str().unwrap());
    let out = Command::new("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A process table whose processes hold the given stdouts.
fn table(dir: &Path, stdouts: &[&str]) -> std::path::PathBuf {
    let proc = dir.join("proc");
    let pids = stdouts
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("{}", 100 + i), *s));
    for (name, target) in std::iter::once(("self".to_owned(), "/dev/null")).chain(pids) {
        let fds = proc.join(name).join("fd");
        fs::create_dir_all(&fds).unwrap();
        symlink(target, fds.join("1")).unwrap();
    }
    proc
}

#[test]
fn a_shell_holding_an_output_file_is_running() {
    let t = TempDir::new().unwrap();
    let proc = table(
        t.path(),
        &["/tmp/x.log", "/tmp/claude-1/p/s/tasks/b1.output"],
    );
    assert_eq!(check(&proc), "running");
}

#[test]
fn no_shell_holding_an_output_file_is_none() {
    let t = TempDir::new().unwrap();
    let proc = table(t.path(), &["/tmp/x.log", "/tmp/tasks/b1.log"]);
    assert_eq!(check(&proc), "none");
}

#[test]
fn a_real_process_holding_an_output_file_is_found() {
    let t = TempDir::new().unwrap();
    let output = t.path().join("s/tasks/b1.output");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    let stdout = fs::File::create(output).unwrap();
    let _build = moor_testkit::ChildGuard::new(
        Command::new("sleep")
            .arg("30")
            .stdout(Stdio::from(stdout))
            .spawn()
            .unwrap(),
    );
    assert_eq!(check(Path::new("/proc")), "running");
}

#[test]
fn a_process_table_it_cannot_read_is_unknown() {
    let t = TempDir::new().unwrap();
    assert_eq!(check(&t.path().join("no-proc")), "unknown");
}

#[test]
fn the_claude_check_looks_through_the_process_table() {
    let script = Claude.wait_check_script().unwrap();
    assert!(script.contains("p='/proc'"), "{script}");
}

fn check_only_local_work_is_marked_local(core: &hc::Core, path: &str) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let piece =
        |kind: &str| json!({"id": "a", "type": kind, "status": "running", "description": kind});
    let marked = |pieces: Vec<Value>| {
        let stop = json!({"background_tasks": pieces});
        run_hook_on(&c, "Stop", Some(&stop.to_string()), Some(path));
        note_of(core).background_local
    };
    assert!(marked(vec![piece("shell"), piece("subagent")]));
    assert!(!marked(vec![piece("shell"), piece("monitor")]));
    assert!(!marked(vec![piece("cloud session")]));
    assert!(!marked(vec![json!({"id": "c3"})]));
    let done = json!({"id": "b", "type": "cloud session", "status": "completed"});
    assert!(marked(vec![piece("shell"), done]));
}

#[test]
fn only_local_work_is_marked_local() {
    on_host(check_only_local_work_is_marked_local);
}

#[test]
fn only_local_work_is_marked_local_with_python3_only() {
    forced_with_core(Interp::Python, check_only_local_work_is_marked_local);
}

#[test]
fn only_local_work_is_marked_local_with_node_only() {
    forced_with_core(Interp::Node, check_only_local_work_is_marked_local);
}
