//! The hook marks work the Box can see, and the wait check looks for that work's processes.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::super::{wait, Claude};
use super::{ctx, forced, install, run_hook_on, state_file, Interp};
use moor_core::seam::agent_adapter::AgentAdapter;

const LOCAL: &str = r#"{"state":"idle","background":["ci"],"background_local":true,"ts":1}"#;

fn check(hook_file: &Path, proc: &Path) -> String {
    let script = wait::script(hook_file.to_str().unwrap(), proc.to_str().unwrap());
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

fn report(dir: &Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("agent-state.json");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_shell_holding_an_output_file_is_running() {
    let t = TempDir::new().unwrap();
    let proc = table(
        t.path(),
        &["/tmp/x.log", "/tmp/claude-1/p/s/tasks/b1.output"],
    );
    assert_eq!(check(&report(t.path(), LOCAL), &proc), "running");
}

#[test]
fn no_shell_holding_an_output_file_is_none() {
    let t = TempDir::new().unwrap();
    let proc = table(t.path(), &["/tmp/x.log", "/tmp/tasks/b1.log"]);
    assert_eq!(check(&report(t.path(), LOCAL), &proc), "none");
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
    assert_eq!(
        check(&report(t.path(), LOCAL), Path::new("/proc")),
        "running"
    );
}

#[test]
fn work_the_box_cannot_see_is_unknown() {
    let t = TempDir::new().unwrap();
    let proc = table(t.path(), &[]);
    for text in [
        r#"{"state":"idle","background":["cloud"],"background_local":false,"ts":1}"#,
        r#"{"state":"idle","background":["from an older hook"],"ts":1}"#,
    ] {
        assert_eq!(check(&report(t.path(), text), &proc), "unknown", "{text}");
    }
    assert_eq!(check(&t.path().join("gone.json"), &proc), "unknown");
}

#[test]
fn a_process_table_it_cannot_read_is_unknown() {
    let t = TempDir::new().unwrap();
    let hook = report(t.path(), LOCAL);
    assert_eq!(check(&hook, &t.path().join("no-proc")), "unknown");
}

#[test]
fn the_claude_check_reads_the_hook_file_it_is_given() {
    let hook = moor_core::name::BoxPath::under("/work/b", ".moor/agent-state.json").unwrap();
    let script = Claude.wait_check_script(&hook).unwrap();
    assert!(
        script.contains("/work/b/.moor/agent-state.json"),
        "{script}"
    );
    assert!(script.contains("p='/proc'"), "{script}");
}

fn check_only_local_work_is_marked_local(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    install(&Claude.hook_writer(&c).unwrap());
    let piece =
        |kind: &str| json!({"id": "a", "type": kind, "status": "running", "description": kind});
    let marked = |pieces: Vec<Value>| {
        let stop = json!({"background_tasks": pieces});
        run_hook_on(&c, "idle", "Stop", Some(&stop.to_string()), path);
        serde_json::from_slice::<Value>(&state_file(&c)).unwrap()["background_local"].clone()
    };
    assert_eq!(marked(vec![piece("shell"), piece("subagent")]), json!(true));
    assert_eq!(marked(vec![piece("shell"), piece("monitor")]), json!(false));
    assert_eq!(marked(vec![piece("cloud session")]), json!(false));
    assert_eq!(marked(vec![json!({"id": "c3"})]), json!(false));
    let done = json!({"id": "b", "type": "cloud session", "status": "completed"});
    assert_eq!(marked(vec![piece("shell"), done]), json!(true));
}

#[test]
fn only_local_work_is_marked_local_with_python3_only() {
    forced(Interp::Python, check_only_local_work_is_marked_local);
}

#[test]
fn only_local_work_is_marked_local_with_node_only() {
    forced(Interp::Node, check_only_local_work_is_marked_local);
}
