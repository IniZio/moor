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

/// A setup-token is `sk-ant-` then letters, digits, `-` and `_`.
#[test]
fn a_claude_token_has_the_shape_setup_token_prints() {
    let ok = |t: &str| super::check_claude_token(&Secret::new(t.into())).is_ok();
    assert!(ok("sk-ant-oat01-abc_DEF-123"));
    for bad in ["", "xoxb-123", "sk-ant-has space", "sk-ant-é"] {
        assert!(!ok(bad), "{bad:?}");
    }
    assert!(!ok(&format!("sk-ant-{}", "a".repeat(512))));
}

/// A GitHub token is one visible ASCII word.
#[test]
fn a_github_token_is_one_visible_word() {
    let ok = |t: &str| super::check_github_token(&Secret::new(t.into())).is_ok();
    assert!(ok("gho_16C7e42F292c6912E7710c838347Ae178B4a"));
    for bad in ["", "two words", "tab\there"] {
        assert!(!ok(bad), "{bad:?}");
    }
}
