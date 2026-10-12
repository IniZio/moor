//! Tests for the setup script, run under `sh` in a scratch directory.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::setup_script;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moor-setup-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn run(&self, steps: &[&str]) -> Option<i32> {
        let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
        Command::new("sh")
            .arg("-c")
            .arg(setup_script(&steps))
            .current_dir(&self.0)
            .output()
            .unwrap()
            .status
            .code()
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.0.join(rel)).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
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
fn steps_share_one_shell() {
    let s = Scratch::new("shared");
    let code = s.run(&["X=kept", "echo $X > out"]);
    assert_eq!(code, Some(0));
    assert_eq!(s.read("out"), "kept\n");
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
