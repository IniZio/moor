//! Tests for shell quoting and command lines.

use std::process::Command;

use super::{command_line, quote};

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
