//! Tests for `Env`.

use super::Env;
use std::path::{Path, PathBuf};

/// Build an env from literal pairs.
fn env(pairs: &[(&str, &str)]) -> Env {
    Env::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
}

#[test]
fn explicit_state_dir_wins() {
    let e = env(&[
        ("MOOR_STATE_DIR", "/a"),
        ("XDG_STATE_HOME", "/x"),
        ("HOME", "/h"),
    ]);
    assert_eq!(e.state_dir(), Some(PathBuf::from("/a")));
}

#[test]
fn xdg_beats_home() {
    let e = env(&[("XDG_STATE_HOME", "/x"), ("HOME", "/h")]);
    assert_eq!(e.state_dir(), Some(PathBuf::from("/x/moor")));
}

#[test]
fn home_fallback() {
    let e = env(&[("HOME", "/h")]);
    assert_eq!(e.state_dir(), Some(PathBuf::from("/h/.local/state/moor")));
}

#[test]
fn nothing_set_is_none() {
    assert_eq!(env(&[]).state_dir(), None);
}

#[test]
fn empty_values_are_unset() {
    let e = env(&[
        ("MOOR_STATE_DIR", ""),
        ("XDG_STATE_HOME", ""),
        ("HOME", "/h"),
    ]);
    assert_eq!(e.state_dir(), Some(PathBuf::from("/h/.local/state/moor")));
    assert_eq!(env(&[("HOME", "")]).state_dir(), None);
}

#[test]
fn with_state_dir_overrides() {
    let e = env(&[("MOOR_STATE_DIR", "/a"), ("HOME", "/h")]).with_state_dir(Path::new("/b"));
    assert_eq!(e.state_dir(), Some(PathBuf::from("/b")));
    assert_eq!(e.get("MOOR_STATE_DIR"), Some("/b"));
}

#[test]
fn get_returns_value() {
    let e = env(&[("K", "v")]);
    assert_eq!(e.get("K"), Some("v"));
    assert_eq!(e.get("missing"), None);
}
