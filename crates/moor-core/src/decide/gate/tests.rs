//! Tests for the gate verdict rules.

use super::{decide, GateError, GateQuery, GateRun};
use crate::name::RunId;
use crate::response::ErrorKind;
use crate::seam::clock::Ts;

const SHA_A: &str = "abc1234567890abcdef1234567890abcdef12345";
const SHA_B: &str = "abc1239999999999999999999999999999999999";

fn run(id: i64, sha: &str, exit: Option<i32>, at: i64) -> GateRun {
    GateRun {
        id: RunId::new(id),
        sha: sha.to_owned(),
        name: "test".to_owned(),
        command: "make test".to_owned(),
        exit,
        interrupted: false,
        recorded_at: Ts::from_millis(at),
    }
}

fn query<'a>(sha: &'a str, command: Option<&'a str>) -> GateQuery<'a> {
    GateQuery {
        sha,
        name: "test",
        command,
    }
}

#[test]
fn short_or_non_hex_sha_is_usage() {
    let runs = [run(1, SHA_A, Some(0), 1)];
    for sha in ["abc", "", "abcg", "abc1 "] {
        let err = decide(&query(sha, None), &runs).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage, "{sha:?}");
    }
}

#[test]
fn sha_case_is_ignored() {
    let runs = [run(1, SHA_A, Some(0), 1)];
    let v = decide(&query("ABC1", None), &runs).unwrap();
    assert_eq!(v.sha, SHA_A);
    assert!(v.ok);
}

#[test]
fn unknown_short_prefix_is_usage() {
    let runs = [run(1, SHA_A, Some(0), 1)];
    let err = decide(&query("dead12", None), &runs).unwrap_err();
    assert!(matches!(err, GateError::Usage(_)));
}

#[test]
fn unknown_long_prefix_is_failed() {
    let runs = [run(1, SHA_A, Some(0), 1)];
    let err = decide(&query("dead123", None), &runs).unwrap_err();
    assert!(matches!(err, GateError::Failed(_)));
    assert_eq!(err.kind(), ErrorKind::Failed);
}

#[test]
fn prefix_matching_two_shas_is_usage() {
    let runs = [run(1, SHA_A, Some(0), 1), run(2, SHA_B, Some(0), 2)];
    let err = decide(&query("abc123", None), &runs).unwrap_err();
    assert!(matches!(err, GateError::Usage(_)));
    assert!(decide(&query("abc1234", None), &runs).is_ok());
}

#[test]
fn passing_run_beats_newer_failing_run() {
    let runs = [run(1, SHA_A, Some(0), 10), run(2, SHA_A, Some(1), 20)];
    let v = decide(&query(SHA_A, None), &runs).unwrap();
    assert!(v.ok);
    assert_eq!(v.exit, Some(0));
    assert_eq!(v.recorded_at, Some(Ts::from_millis(10)));
}

#[test]
fn without_a_pass_the_latest_run_is_reported() {
    let runs = [run(1, SHA_A, Some(2), 10), run(2, SHA_A, Some(1), 20)];
    let v = decide(&query(SHA_A, None), &runs).unwrap();
    assert!(!v.ok);
    assert_eq!(v.exit, Some(1));
}

#[test]
fn command_filter_matches_exact_text() {
    let mut other = run(2, SHA_A, Some(1), 20);
    other.command = "make lint".to_owned();
    let runs = [run(1, SHA_A, Some(0), 10), other];
    let v = decide(&query(SHA_A, Some("make lint")), &runs).unwrap();
    assert!(!v.ok);
    assert_eq!(v.command.as_deref(), Some("make lint"));
    let v = decide(&query(SHA_A, Some("make")), &runs).unwrap();
    assert_eq!(v.command, None);
}

#[test]
fn interrupted_run_never_passes() {
    let mut r = run(1, SHA_A, Some(0), 10);
    r.interrupted = true;
    let v = decide(&query(SHA_A, None), &[r]).unwrap();
    assert!(!v.ok);
}

#[test]
fn run_without_exit_never_passes() {
    let v = decide(&query(SHA_A, None), &[run(1, SHA_A, None, 10)]).unwrap();
    assert!(!v.ok);
    assert_eq!(v.exit, None);
    assert!(v.recorded_at.is_some());
}

#[test]
fn known_sha_without_runs_of_the_name_is_not_ok() {
    let mut r = run(1, SHA_A, Some(0), 10);
    r.name = "lint".to_owned();
    let v = decide(&query("abc1234", None), &[r]).unwrap();
    assert!(!v.ok);
    assert_eq!(v.exit, None);
    assert_eq!(v.command, None);
    assert_eq!(v.recorded_at, None);
    assert_eq!(v.sha, SHA_A);
}
