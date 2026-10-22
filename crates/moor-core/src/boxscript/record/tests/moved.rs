//! A record run whose checkout moved, or whose git state cannot be read afterwards, fails.

use super::super::{parse_head, HEAD_FILE, HEAD_MOVED, MOVED_FILE};
use super::{probe_sandbox, probed_dirty, Out, Sandbox, ZERO};
use crate::Sha;

/// Runs `cmd` as a record run of the committed HEAD; asserts it failed as moved.
fn assert_moved(sb: &Sandbox, cmd: &str, head: &Sha) -> Out {
    let out = sb.run_at(&[], cmd, head);
    assert_ne!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains(HEAD_MOVED), "{}", out.stderr);
    assert!(sb.exists(MOVED_FILE));
    out
}

#[test]
fn a_commit_during_the_record_run_fails_it_as_moved() {
    let sb = probe_sandbox("m-commit");
    let head = sb.head();
    assert_moved(&sb, "echo x > kept.txt && git commit -qam y", &head);
}

#[test]
fn an_untracked_file_left_by_the_record_run_passes_at_the_probed_sha() {
    let sb = probe_sandbox("m-new");
    assert_eq!(probed_dirty(&sb), 0);
    let probed = Sha::parse(&parse_head(&sb.read(HEAD_FILE)).unwrap().0).unwrap();
    let out = sb.run_at(&[], "mkdir -p cov && touch left.txt cov/out", &probed);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!sb.exists(MOVED_FILE));
    assert_eq!(sb.head().as_str(), probed.as_str());
}

#[test]
fn a_tracked_change_by_the_record_run_fails_it_as_moved() {
    for (name, cmd) in [
        ("m-edit", "echo x > kept.txt"),
        ("m-rm", "rm gone.txt"),
        ("m-stage", "echo x > kept.txt && git add kept.txt"),
    ] {
        let sb = probe_sandbox(name);
        assert_moved(&sb, cmd, &sb.head());
    }
}

#[test]
fn a_head_other_than_the_probed_one_fails_the_run_as_moved() {
    let sb = probe_sandbox("m-other");
    assert_moved(&sb, "true", &Sha::parse(ZERO).unwrap());
}

#[test]
fn a_moved_run_keeps_the_command_failure_code() {
    let sb = probe_sandbox("m-code");
    let head = sb.head();
    assert_eq!(
        assert_moved(&sb, "echo x > kept.txt; exit 4", &head).code,
        4
    );
}

#[test]
fn a_clean_run_after_a_moved_one_clears_the_moved_file() {
    let sb = probe_sandbox("m-clear");
    let head = sb.head();
    assert_moved(&sb, "true", &Sha::parse(ZERO).unwrap());
    let out = sb.run_at(&[], "true", &head);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!sb.exists(MOVED_FILE));
}

#[test]
fn ignored_output_of_the_record_run_does_not_fail_it() {
    let sb = probe_sandbox("m-ign");
    let head = sb.head();
    let out = sb.run_at(&[], "touch out.log .moor/x", &head);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!sb.exists(MOVED_FILE));
}

#[test]
fn a_git_failure_after_the_record_run_fails_it_as_moved() {
    let sb = probe_sandbox("m-index");
    let head = sb.head();
    assert_moved(&sb, "printf 'not an index' > .git/index", &head);
}
