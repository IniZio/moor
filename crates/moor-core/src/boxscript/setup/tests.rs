//! Tests for the setup script, run under `sh` in a scratch directory.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::{setup_script, BOX_TOOLS};

struct Scratch(tempfile::TempDir);

impl Scratch {
    fn new(name: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(&format!("moor-setup-{name}-"))
                .tempdir()
                .unwrap(),
        )
    }

    fn run(&self, steps: &[&str]) -> Option<i32> {
        let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
        Command::new("sh")
            .arg("-c")
            .arg(setup_script(&steps))
            .current_dir(self.0.path())
            .output()
            .unwrap()
            .status
            .code()
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.0.path().join(rel)).unwrap_or_default()
    }
}

#[test]
fn steps_run_in_order() {
    let s = Scratch::new("order");
    let code = s.run(&[
        "echo one >> order",
        "echo two >> order",
        "echo three >> order",
    ]);
    assert_eq!(code, Some(0));
    assert_eq!(s.read("order"), "one\ntwo\nthree\n");
}

#[test]
fn variables_do_not_leak_between_steps() {
    let s = Scratch::new("isolated");
    let code = s.run(&["X=kept", "echo \"[$X]\" > out"]);
    assert_eq!(code, Some(0));
    assert_eq!(s.read("out"), "[]\n");
}

#[test]
fn failing_plain_step_stops_later_steps() {
    let s = Scratch::new("plain");
    let code = s.run(&["false", "echo c >> ran"]);
    assert_eq!(code, Some(1));
    assert_eq!(s.read("ran"), "");
    assert_eq!(s.read(".moor/setup.rc").trim(), "1");
}

#[test]
fn failed_and_list_still_stops_later_steps() {
    let s = Scratch::new("andlist");
    let code = s.run(&["false && true", "touch reached"]);
    assert_eq!(code, Some(1));
    assert!(!s.0.path().join("reached").exists());
}

#[test]
fn exit_zero_step_does_not_skip_later_steps() {
    let s = Scratch::new("exit0");
    let code = s.run(&["echo a >> ran", "exit 0", "echo c >> ran"]);
    assert_eq!(code, Some(0));
    assert_eq!(s.read("ran"), "a\nc\n");
}

#[test]
fn disabling_errexit_does_not_stop_failures_from_stopping() {
    let s = Scratch::new("noerrexit");
    let code = s.run(&["set +e", "false", "echo c >> ran"]);
    assert_eq!(code, Some(1));
    assert_eq!(s.read("ran"), "");
}

#[test]
fn unbalanced_step_cannot_escape_logging() {
    let s = Scratch::new("escape");
    let code = s.run(&[") ; touch escaped ; (", "true"]);
    assert_ne!(code, Some(0));
    assert!(!s.0.path().join("escaped").exists());
    let rc = s.read(".moor/setup.rc");
    assert_ne!(rc.trim(), "0");
    assert!(s
        .read(".moor/setup.log")
        .contains("moor setup: step 1 exited"));
}

#[test]
fn failing_step_stops_later_steps() {
    let s = Scratch::new("stops");
    let code = s.run(&["echo a >> ran", "exit 3", "echo c >> ran"]);
    assert_eq!(code, Some(3));
    assert_eq!(s.read("ran"), "a\n");
}

#[test]
fn log_holds_step_output_and_announcements() {
    let s = Scratch::new("log");
    s.run(&["echo hello", "echo oops >&2"]);
    let log = s.read(".moor/setup.log");
    assert!(log.contains("moor setup: $ echo hello\nhello\n"), "{log}");
    assert!(log.contains("moor setup: $ echo oops >&2\noops\n"), "{log}");
}

#[test]
fn failing_step_is_logged_with_number_and_rc() {
    let s = Scratch::new("failed");
    s.run(&["true", "exit 3"]);
    let log = s.read(".moor/setup.log");
    assert!(log.contains("moor setup: step 2 exited 3"), "{log}");
}

#[test]
fn rc_file_holds_failing_exit_code() {
    let s = Scratch::new("rc");
    s.run(&["exit 3"]);
    assert_eq!(s.read(".moor/setup.rc").trim(), "3");
}

#[test]
fn all_passing_steps_exit_zero_with_rc_zero() {
    let s = Scratch::new("pass");
    assert_eq!(s.run(&["true", "echo ok"]), Some(0));
    assert_eq!(s.read(".moor/setup.rc").trim(), "0");
    assert!(!s.read(".moor/setup.log").contains("exited"));
}

#[test]
fn step_text_with_quotes_is_announced_verbatim() {
    let s = Scratch::new("quotes");
    s.run(&["echo \"it's $HOME\" > /dev/null"]);
    let log = s.read(".moor/setup.log");
    assert!(
        log.contains("moor setup: $ echo \"it's $HOME\" > /dev/null\n"),
        "{log}"
    );
}

#[test]
fn no_steps_still_writes_rc_zero() {
    let s = Scratch::new("empty");
    assert_eq!(s.run(&[]), Some(0));
    assert_eq!(s.read(".moor/setup.rc").trim(), "0");
}

#[test]
fn script_checks_each_box_tool_without_failing() {
    let script = setup_script(&[]);
    for tool in BOX_TOOLS {
        let line = format!("command -v {tool} >/dev/null 2>&1 || printf");
        assert!(script.contains(&line), "no guarded check for {tool}");
    }
}

#[test]
fn missing_tools_warn_in_log_and_stderr_without_changing_rc() {
    let s = Scratch::new("tools");
    let bin = s.0.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for tool in ["sh", "mkdir", "tee", "cat"] {
        let src = ["/bin", "/usr/bin"]
            .iter()
            .map(|d| PathBuf::from(d).join(tool))
            .find(|p| p.exists())
            .unwrap();
        std::os::unix::fs::symlink(src, bin.join(tool)).unwrap();
    }
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(setup_script(&["true".to_owned()]))
        .env("PATH", &bin)
        .current_dir(&s.0)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(s.read(".moor/setup.rc").trim(), "0");
    let log = s.read(".moor/setup.log");
    let shown = String::from_utf8_lossy(&out.stdout);
    for tool in BOX_TOOLS.iter().filter(|t| **t != "sh") {
        let warning = format!("moor: warning: the Box has no {tool};");
        assert!(log.contains(&warning), "log lacks {tool}");
        assert!(shown.contains(&warning), "output lacks {tool}");
    }
    assert!(!log.contains("has no sh;"));
}
