//! Tests for upstream configuration and credential formatting.

use moor_core::proxy_gate::{Inject, RouteId};
use moor_core::Secret;

use super::{auth_value, base64, Upstreams};
use crate::env::Env;

/// Env with the given variables.
fn env(pairs: &[(&str, &str)]) -> Env {
    Env::from_pairs(pairs.iter().copied())
}

/// Base of `route` from `ups`.
fn base<'a>(ups: &'a Upstreams, route: &str) -> &'a str {
    ups.base(&RouteId::new(route)).unwrap()
}

/// Header value as text.
fn text(s: Option<Secret>) -> String {
    String::from_utf8(s.unwrap().expose().to_vec()).unwrap()
}

/// Defaults point at the production hosts.
#[test]
fn defaults() {
    let ups = Upstreams::from_env(&env(&[])).unwrap();
    assert_eq!(base(&ups, "model"), "https://api.anthropic.com");
    assert_eq!(base(&ups, "forge.git"), "https://github.com");
    assert_eq!(base(&ups, "forge.api"), "https://api.github.com");
    assert!(ups.base(&RouteId::new("other")).is_none());
}

/// Overrides apply under `MOOR_TEST=1`.
#[test]
fn override_applies_in_test_mode() {
    let e = env(&[
        ("MOOR_TEST", "1"),
        ("MOOR_PROXY_UPSTREAM_MODEL", "https://m.example/"),
    ]);
    let ups = Upstreams::from_env(&e).unwrap();
    assert_eq!(base(&ups, "model"), "https://m.example");
    assert_eq!(base(&ups, "forge.git"), "https://github.com");
}

/// Overrides are ignored when `MOOR_TEST` is unset.
#[test]
fn override_ignored_without_test_mode() {
    let e = env(&[("MOOR_PROXY_UPSTREAM_MODEL", "http://evil.example")]);
    let ups = Upstreams::from_env(&e).unwrap();
    assert_eq!(base(&ups, "model"), "https://api.anthropic.com");
}

/// Overrides are ignored when `MOOR_TEST` is not exactly 1.
#[test]
fn override_ignored_with_test_zero() {
    let e = env(&[
        ("MOOR_TEST", "0"),
        ("MOOR_PROXY_UPSTREAM_MODEL", "http://evil.example"),
    ]);
    let ups = Upstreams::from_env(&e).unwrap();
    assert_eq!(base(&ups, "model"), "https://api.anthropic.com");
}

/// An http override is accepted in test mode.
#[test]
fn http_override_accepted_in_test_mode() {
    let e = env(&[
        ("MOOR_TEST", "1"),
        ("MOOR_PROXY_UPSTREAM_FORGE_API", "http://127.0.0.1:9"),
    ]);
    let ups = Upstreams::from_env(&e).unwrap();
    assert_eq!(base(&ups, "forge.api"), "http://127.0.0.1:9");
}

/// An empty override counts as unset.
#[test]
fn empty_override_is_unset() {
    let e = env(&[("MOOR_TEST", "1"), ("MOOR_PROXY_UPSTREAM_MODEL", "")]);
    let ups = Upstreams::from_env(&e).unwrap();
    assert_eq!(base(&ups, "model"), "https://api.anthropic.com");
}

/// Route ids map to upper-case names with underscores.
#[test]
fn env_name_mapping() {
    let e = env(&[
        ("MOOR_PROXY_UPSTREAM_FORGE_GIT", "https://g.example"),
        ("MOOR_PROXY_UPSTREAM_MODEL", "https://m.example"),
    ]);
    assert_eq!(e.proxy_upstream("forge.git"), Some("https://g.example"));
    assert_eq!(e.proxy_upstream("model"), Some("https://m.example"));
    assert_eq!(e.proxy_upstream("forge.api"), None);
}

/// `MOOR_TEST` is true only for exactly 1.
#[test]
fn test_mode_is_exact() {
    assert!(env(&[("MOOR_TEST", "1")]).test_mode());
    assert!(!env(&[("MOOR_TEST", "0")]).test_mode());
    assert!(!env(&[("MOOR_TEST", "true")]).test_mode());
    assert!(!env(&[]).test_mode());
}

/// http is rejected unless allowed.
#[test]
fn new_rejects_http() {
    let r = Upstreams::new([("model".to_owned(), "http://a.example".to_owned())], false);
    assert!(r.is_err());
}

/// Other schemes fail even when http is allowed.
#[test]
fn new_rejects_ftp() {
    let r = Upstreams::new([("model".to_owned(), "ftp://a.example".to_owned())], true);
    assert!(r.is_err());
}

/// Values without a scheme or host are rejected.
#[test]
fn new_rejects_bad_urls() {
    for bad in ["a.example", "https://", "https:///x", "https://a b"] {
        let r = Upstreams::new([("model".to_owned(), bad.to_owned())], true);
        assert!(r.is_err(), "{bad}");
    }
}

/// Trailing slashes are trimmed and `url()` joins path and query.
#[test]
fn trailing_slash_and_url_join() {
    let ups = Upstreams::new(
        [("model".to_owned(), "https://a.example//".to_owned())],
        false,
    )
    .unwrap();
    let id = RouteId::model();
    assert_eq!(ups.base(&id), Some("https://a.example"));
    assert_eq!(
        ups.url(&id, "/v1/x?y=1").as_deref(),
        Some("https://a.example/v1/x?y=1")
    );
    assert!(ups.url(&RouteId::new("nope"), "/").is_none());
}

/// Encoder matches the RFC 4648 vectors.
#[test]
fn base64_rfc4648_vectors() {
    for (i, o) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(i.as_bytes()), o);
    }
}

/// Bearer prefixes the secret.
#[test]
fn bearer_value() {
    let v = auth_value(Inject::Bearer, &Secret::new(b"tok".to_vec()));
    assert_eq!(text(v), "Bearer tok");
}

/// Basic encodes user and secret.
#[test]
fn basic_value() {
    let inject = Inject::Basic {
        user: "x-access-token",
    };
    let v = auth_value(inject, &Secret::new(b"abc".to_vec()));
    assert_eq!(text(v), "Basic eC1hY2Nlc3MtdG9rZW46YWJj");
}

/// No injection yields no header.
#[test]
fn none_value() {
    assert!(auth_value(Inject::None, &Secret::new(b"abc".to_vec())).is_none());
}
