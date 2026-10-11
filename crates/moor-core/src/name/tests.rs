//! Tests for name validation.

use super::{BoxPath, Sha, TaskName};

/// Valid task names parse.
#[test]
fn task_name_valid() {
    assert_eq!(TaskName::parse("a").unwrap().as_str(), "a");
    assert!(TaskName::parse("fix-bug-9").is_ok());
    assert!(TaskName::parse("9lives").is_ok());
    assert!(TaskName::parse(&"a".repeat(40)).is_ok());
}

/// Invalid task names are rejected.
#[test]
fn task_name_invalid() {
    assert!(TaskName::parse("").is_err());
    assert!(TaskName::parse("Abc").is_err());
    assert!(TaskName::parse("-abc").is_err());
    assert!(TaskName::parse("a_b").is_err());
    assert!(TaskName::parse(&"a".repeat(41)).is_err());
}

/// Sha length and charset limits hold.
#[test]
fn sha_rules() {
    assert!(Sha::parse("abc").is_err());
    assert!(Sha::parse("abcd").is_ok());
    assert!(Sha::parse(&"a".repeat(40)).is_ok());
    assert!(Sha::parse(&"a".repeat(41)).is_err());
    assert!(Sha::parse("abcg").is_err());
    assert_eq!(Sha::parse("ABCD1234").unwrap().as_str(), "abcd1234");
}

/// `BoxPath::under` checks both parts and joins with `/`.
#[test]
fn box_path_under() {
    assert!(BoxPath::under("work", "a").is_err());
    assert!(BoxPath::under("/w", "../a").is_err());
    assert!(BoxPath::under("/w", "a/../b").is_err());
    assert!(BoxPath::under("/w", "/a").is_err());
    assert!(BoxPath::under("/w", "").is_err());
    assert_eq!(BoxPath::under("/w", "a/b").unwrap().as_str(), "/w/a/b");
    assert_eq!(BoxPath::under("/w/", "a").unwrap().as_str(), "/w/a");
}
