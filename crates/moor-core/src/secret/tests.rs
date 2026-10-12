//! Tests for `Secret` and `Digest`.

use super::Secret;

/// The same bytes expose and digest identically.
#[test]
fn expose_and_digest_are_stable() {
    let a = Secret::new(b"hunter2".to_vec());
    let b = Secret::new(b"hunter2".to_vec());
    assert_eq!(a.expose(), b"hunter2");
    assert_eq!(a.digest(), b.digest());
    assert_eq!(a.digest().as_str().len(), 16);
    assert!(a
        .digest()
        .as_str()
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
}

/// Different bytes give different digests.
#[test]
fn digest_differs_for_different_secrets() {
    let a = Secret::new(b"one".to_vec());
    let b = Secret::new(b"two".to_vec());
    assert_ne!(a.digest(), b.digest());
}

/// The digest of empty input is the known sha256 prefix.
#[test]
fn digest_matches_known_sha256_prefix() {
    let s = Secret::new(Vec::new());
    assert_eq!(s.digest().as_str(), "e3b0c44298fc1c14");
}
