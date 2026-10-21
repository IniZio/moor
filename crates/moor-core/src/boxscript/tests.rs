//! Tests for shell quoting and command lines.

use std::ffi::OsStr;
use std::process::Command;

use super::{command_line, quote, shell_argv};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn quote_wraps_plain_text() {
    assert_eq!(quote("abc"), "'abc'");
}

#[test]
fn quote_wraps_empty_string() {
    assert_eq!(quote(""), "''");
}

#[test]
fn quote_escapes_single_quote() {
    assert_eq!(quote("it's"), "'it'\\''s'");
}

#[test]
fn command_line_single_element_runs_through_sh() {
    assert_eq!(
        command_line(&argv(&["echo hi && true"])),
        "sh -c 'echo hi && true'"
    );
}

#[test]
fn command_line_keeps_spaced_argument_as_one_word() {
    assert_eq!(command_line(&argv(&["echo", "a b"])), "'echo' 'a b'");
}

#[test]
fn command_line_escapes_quote_in_argument() {
    assert_eq!(command_line(&argv(&["echo", "it's"])), "'echo' 'it'\\''s'");
}

#[test]
fn command_line_of_empty_argv_is_empty() {
    assert_eq!(command_line(&[]), "");
}

#[test]
fn command_line_round_trips_awkward_arguments_under_sh() {
    let line = command_line(&argv(&["printf", "%s|", "a b", "it's"]));
    let out = Command::new("sh").arg("-c").arg(line).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a b|it's|");
}

/// Runs `shell_argv(script)` with `path` as `PATH`, returning its stdout.
fn shell_out(script: &str, path: &OsStr) -> String {
    let words = shell_argv(script);
    let (program, rest) = words.split_first().unwrap();
    let out = Command::new(program)
        .args(rest)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn shell_text_runs_under_bash_when_the_box_has_it() {
    let path = std::env::var_os("PATH").unwrap();
    let out = shell_out("set -o pipefail; false | true; echo \"rc=$? 'q'\"", &path);
    assert_eq!(out, "rc=1 'q'\n");
}

#[test]
fn shell_text_falls_back_to_sh_without_bash() {
    let dir = std::env::temp_dir().join(format!("moor-core-sh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sh = std::fs::canonicalize(which_sh()).unwrap();
    let link = dir.join("sh");
    std::fs::remove_file(&link).ok();
    std::os::unix::fs::symlink(sh, &link).unwrap();
    let out = shell_out("echo \"${BASH_VERSION:-none} $0\"", dir.as_os_str());
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out, "none sh\n");
}

/// The `sh` on the test's `PATH`.
fn which_sh() -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("sh"))
        .find(|p| p.is_file())
        .unwrap()
}
