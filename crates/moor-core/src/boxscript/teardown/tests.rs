//! Tests for the teardown script, run under `sh` in a scratch directory.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::teardown_script;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moor-teardown-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn run(&self, steps: &[&str]) -> Option<i32> {
        let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
        Command::new("sh")
            .arg("-c")
            .arg(teardown_script(&steps))
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
fn steps_run_in_order_and_exit_zero() {
    let s = Scratch::new("order");
    assert_eq!(s.run(&["echo a >> out", "echo b >> out"]), Some(0));
    assert_eq!(s.read("out"), "a\nb\n");
    assert_eq!(s.read(".moor/teardown.rc").trim(), "0");
}

#[test]
fn failing_step_stops_later_steps_and_sets_the_exit_code() {
    let s = Scratch::new("fail");
    assert_eq!(
        s.run(&["echo a >> out", "exit 3", "echo c >> out"]),
        Some(3)
    );
    assert_eq!(s.read("out"), "a\n");
    assert_eq!(s.read(".moor/teardown.rc").trim(), "3");
}

#[test]
fn log_holds_announcements_output_and_the_failing_step() {
    let s = Scratch::new("log");
    s.run(&["echo hello", "exit 4"]);
    let log = s.read(".moor/teardown.log");
    assert!(
        log.contains("moor teardown: $ echo hello\nhello\n"),
        "{log}"
    );
    assert!(log.contains("moor teardown: step 2 exited 4"), "{log}");
}

#[test]
fn setup_files_are_left_alone() {
    let s = Scratch::new("apart");
    fs::create_dir_all(s.0.join(".moor")).unwrap();
    fs::write(s.0.join(".moor/setup.log"), "kept").unwrap();
    s.run(&["true"]);
    assert_eq!(s.read(".moor/setup.log"), "kept");
}

#[test]
fn no_steps_still_writes_rc_zero() {
    let s = Scratch::new("empty");
    assert_eq!(s.run(&[]), Some(0));
    assert_eq!(s.read(".moor/teardown.rc").trim(), "0");
}

#[test]
fn steps_see_host_tool_dirs_after_original_path() {
    let s = Scratch::new("path");
    let script = teardown_script(&["echo \"PATH=$PATH\"".to_owned()]);
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&s.0)
        .output()
        .unwrap();
    let log = s.read(".moor/teardown.log");
    assert!(
        log.contains("\nPATH=/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin\n"),
        "{log}"
    );
}
