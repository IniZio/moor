//! Tests for the per-Task proxy nonce and failure-reason helpers.

use std::path::Path;

use moor_core::TaskId;

use super::{base64url, log_path, new_nonce, tail, TAIL_LINES};
use crate::testing::rng::FakeRng;

/// Encoding follows RFC 4648 section 5 without padding.
#[test]
fn base64url_known_vectors() {
    assert_eq!(base64url(b""), "");
    assert_eq!(base64url(b"f"), "Zg");
    assert_eq!(base64url(b"ab"), "YWI");
    assert_eq!(base64url(b"foo"), "Zm9v");
    assert_eq!(base64url(b"foob"), "Zm9vYg");
}

/// The URL-safe alphabet replaces `+` and `/` and never pads.
#[test]
fn base64url_uses_url_alphabet() {
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
}

/// A nonce is 32 bytes of base64url text: 43 characters.
#[test]
fn nonce_is_43_url_safe_bytes() {
    let nonce = new_nonce(&FakeRng::default());
    let bytes = nonce.expose();
    assert_eq!(bytes.len(), 43);
    assert!(
        bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_'),
        "unexpected byte in nonce"
    );
}

/// Successive draws from one source yield different nonces.
#[test]
fn nonces_differ_across_draws() {
    let rng = FakeRng::default();
    let a = new_nonce(&rng);
    let b = new_nonce(&rng);
    assert_ne!(a.expose(), b.expose());
}

/// The log lives under `<state>/proxy/<id>.log`.
#[test]
fn log_path_is_under_proxy_dir() {
    assert_eq!(
        log_path(Path::new("/s"), TaskId::new(7)),
        Path::new("/s/proxy/7.log")
    );
}

/// Only the last `TAIL_LINES` lines are returned.
#[test]
fn tail_keeps_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.log");
    let lines: Vec<String> = (1..=25).map(|i| format!("line {i}")).collect();
    let body = lines.join("\n") + "\n";
    std::fs::write(&path, body).unwrap();

    let got = tail(&path);

    let want: Vec<String> = (6..=25).map(|i| format!("line {i}")).collect();
    assert_eq!(TAIL_LINES, 20);
    assert_eq!(got.lines().collect::<Vec<_>>(), want);
}

/// An unreadable log yields an empty reason.
#[test]
fn tail_of_missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(tail(&dir.path().join("absent.log")), "");
}
