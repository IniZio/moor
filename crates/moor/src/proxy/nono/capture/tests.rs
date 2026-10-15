//! Capture backend behaviour: caching, eviction, error text.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moor_core::config::proxy::ProxyConfig;
use moor_core::config::proxy::{default_rules, resolve_rules, ResolvedInject, ResolvedRule};
use moor_core::config::CredentialSource;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo};
use moor_core::Secret;
use nono_proxy::capture::{
    CredentialCaptureBackend, CredentialCaptureMaterial, CredentialCaptureRequest,
};

use super::{sources_for, CredsFn, FetchFn, MoorCapture, RuleSource, CACHE_TTL};
use crate::testing::clock::FakeClock;
use crate::testing::forge::FakeForge;

const SECRET: &str = "s3cr3t-value";

fn clock() -> Arc<FakeClock> {
    Arc::new(FakeClock::new(Ts::from_millis(0)))
}

fn request(rule: &str) -> CredentialCaptureRequest {
    CredentialCaptureRequest {
        credential_name: rule.to_owned(),
        route_id: format!("{rule}__0"),
        request_host: "example.com".to_owned(),
        request_path: "/".to_owned(),
        request_method: "GET".to_owned(),
        session_id: "s".to_owned(),
        cache_scope: "c".to_owned(),
    }
}

/// Fetch closure returning `values` in turn (last repeats) and counting calls.
fn counting(values: Vec<Result<&'static str, &'static str>>) -> (FetchFn, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let fetch: FetchFn = Arc::new(move || {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        match values.get(n).or_else(|| values.last()) {
            Some(Ok(v)) => Ok(Secret::new(v.as_bytes().to_vec())),
            Some(Err(e)) => Err((*e).to_owned()),
            None => Err("none".to_owned()),
        }
    });
    (fetch, calls)
}

fn capture_with(fetch: FetchFn, basic_user: Option<&str>, clock: &Arc<FakeClock>) -> MoorCapture {
    let source = RuleSource {
        config_key: "credentials.K".to_owned(),
        fetch,
        basic_user: basic_user.map(str::to_owned),
    };
    let clock: Arc<dyn Clock> = Arc::clone(clock) as Arc<dyn Clock>;
    MoorCapture::new(BTreeMap::from([("r".to_owned(), source)]), clock)
}

fn secret_of(cap: &MoorCapture, rule: &str) -> Result<(String, String), String> {
    cap.capture(request(rule))
        .map(|r| match r.material {
            CredentialCaptureMaterial::Secret(s) => (s.to_string(), r.metadata.cache_action),
            CredentialCaptureMaterial::Headers(_) => (String::new(), String::new()),
        })
        .map_err(|e| e.reason)
}

#[test]
fn hit_within_ttl_fetches_once() {
    let (fetch, calls) = counting(vec![Ok(SECRET)]);
    let clock = clock();
    let cap = capture_with(fetch, None, &clock);
    assert_eq!(
        secret_of(&cap, "r"),
        Ok((SECRET.to_owned(), "miss".to_owned()))
    );
    clock.sleep(CACHE_TTL / 2);
    assert_eq!(
        secret_of(&cap, "r"),
        Ok((SECRET.to_owned(), "hit".to_owned()))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn refetches_after_ttl() {
    let (fetch, calls) = counting(vec![Ok(SECRET)]);
    let clock = clock();
    let cap = capture_with(fetch, None, &clock);
    secret_of(&cap, "r").unwrap();
    clock.sleep(CACHE_TTL);
    secret_of(&cap, "r").unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn evict_forces_refetch() {
    let (fetch, calls) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None, &clock());
    secret_of(&cap, "r").unwrap();
    cap.evict("r");
    secret_of(&cap, "r").unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn failure_is_not_cached() {
    let (fetch, calls) = counting(vec![Err("boom"), Ok(SECRET)]);
    let cap = capture_with(fetch, None, &clock());
    assert_eq!(
        secret_of(&cap, "r"),
        Err("credentials.K unavailable".to_owned())
    );
    assert!(secret_of(&cap, "r").is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn blank_secret_is_rejected() {
    let (fetch, _) = counting(vec![Ok("  \n")]);
    let cap = capture_with(fetch, None, &clock());
    assert_eq!(
        secret_of(&cap, "r"),
        Err("credentials.K unavailable".to_owned())
    );
}

#[test]
fn non_utf8_secret_names_the_key() {
    let fetch: FetchFn = Arc::new(|| Ok(Secret::new(vec![0xff, 0xfe, b'a'])));
    let cap = capture_with(fetch, None, &clock());
    assert_eq!(
        secret_of(&cap, "r"),
        Err("credentials.K unavailable".to_owned())
    );
}

#[test]
fn unknown_rule_errors() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None, &clock());
    assert_eq!(
        secret_of(&cap, "nope"),
        Err("unknown proxy rule nope".to_owned())
    );
}

#[test]
fn basic_user_prefixes_secret_on_miss_and_hit() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, Some("x-access-token"), &clock());
    let want = format!("x-access-token:{SECRET}");
    assert_eq!(secret_of(&cap, "r").unwrap().0, want);
    assert_eq!(secret_of(&cap, "r").unwrap().0, want);
}

#[test]
fn debug_has_rule_names_and_flags_only() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None, &clock());
    assert!(format!("{cap:?}").contains("(\"r\", false)"));
    secret_of(&cap, "r").unwrap();
    let shown = format!("{cap:?}");
    assert!(shown.contains("(\"r\", true)"));
    assert!(!shown.contains(SECRET));
}

fn repo() -> ForgeRepo {
    ForgeRepo::new("acme", "widgets")
}

fn rule(name: &str, credential: &str, inject: ResolvedInject) -> ResolvedRule {
    ResolvedRule {
        name: name.to_owned(),
        hosts: vec!["example.com".to_owned()],
        credential: credential.to_owned(),
        inject,
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    }
}

fn no_creds() -> CredsFn {
    Arc::new(|name| Ok(Secret::new(format!("cred-{name}").into_bytes())))
}

#[test]
fn forge_rule_uses_repo_token_with_basic_user() {
    let forge: Arc<dyn Forge> = Arc::new(FakeForge::new().with_token(repo(), SECRET.as_bytes()));
    let rules = [rule(
        "git",
        "forge",
        ResolvedInject::BasicAuth {
            user: "x-access-token".to_owned(),
        },
    )];
    let sources = sources_for(&rules, no_creds(), forge, Some(repo()));
    let cap = MoorCapture::new(sources, clock());
    assert_eq!(
        secret_of(&cap, "git").unwrap().0,
        format!("x-access-token:{SECRET}")
    );
}

#[test]
fn forge_rule_without_repo_names_forge_key() {
    let forge: Arc<dyn Forge> = Arc::new(FakeForge::new());
    let rules = [rule(
        "git",
        "forge",
        ResolvedInject::BasicAuth {
            user: "u".to_owned(),
        },
    )];
    let cap = MoorCapture::new(sources_for(&rules, no_creds(), forge, None), clock());
    assert_eq!(
        secret_of(&cap, "git"),
        Err("github.you_token_cmd unavailable".to_owned())
    );
}

#[test]
fn forge_error_text_does_not_leak() {
    let forge: Arc<dyn Forge> =
        Arc::new(FakeForge::new().with_token_error(repo(), ForgeError::Failed(SECRET.to_owned())));
    let rules = [rule(
        "git",
        "forge",
        ResolvedInject::BasicAuth {
            user: "u".to_owned(),
        },
    )];
    let cap = MoorCapture::new(
        sources_for(&rules, no_creds(), forge, Some(repo())),
        clock(),
    );
    let err = secret_of(&cap, "git").unwrap_err();
    assert!(!err.contains(SECRET));
    assert!(err.contains("github.you_token_cmd"));
}

#[test]
fn default_rules_map_to_expected_keys() {
    let creds = BTreeMap::from([(
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        CredentialSource::default(),
    )]);
    assert!(!default_rules().is_empty());
    let rules = resolve_rules(&ProxyConfig::default(), &creds, Some(("acme", "widgets"))).unwrap();
    let forge: Arc<dyn Forge> = Arc::new(FakeForge::new());
    let sources = sources_for(&rules, no_creds(), forge, Some(repo()));
    let keys: Vec<&str> = sources.values().map(|s| s.config_key.as_str()).collect();
    assert!(keys.contains(&"credentials.CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(keys.contains(&"github.you_token_cmd"));
    assert!(sources.values().any(|s| s.basic_user.is_some()));
    assert_eq!(sources.len(), rules.len());
}

fn header_inject(form: &str) -> ResolvedInject {
    ResolvedInject::Header {
        header: "authorization".to_owned(),
        form: form.to_owned(),
    }
}

/// Fetches count after a cached credential met `presented` on a 401.
fn fetches_after(inject: ResolvedInject, user: Option<&str>, presented: &str) -> usize {
    let (fetch, calls) = counting(vec![Ok(SECRET)]);
    let mut cap = capture_with(fetch, user, &clock());
    cap.defer_evictions([("r".to_owned(), inject)]);
    secret_of(&cap, "r").unwrap();
    cap.evict_if_current("r", presented);
    secret_of(&cap, "r").unwrap();
    calls.load(Ordering::SeqCst)
}

#[test]
fn evict_if_current_ignores_a_value_nono_did_not_inject() {
    for bogus in [
        "token bogus",
        "token",
        "",
        "Token s3cr3t-value",
        "token s3cr3t-valuex",
    ] {
        assert_eq!(
            fetches_after(header_inject("token {secret}"), None, bogus),
            1,
            "{bogus}"
        );
    }
}

#[test]
fn evict_if_current_evicts_on_the_injected_header_form() {
    assert_eq!(
        fetches_after(header_inject("token {secret}"), None, "token s3cr3t-value"),
        2
    );
    assert_eq!(
        fetches_after(
            header_inject("Bearer {secret}"),
            None,
            "Bearer s3cr3t-value"
        ),
        2
    );
}

#[test]
fn evict_if_current_evicts_on_the_basic_auth_value() {
    let basic = ResolvedInject::BasicAuth {
        user: "u".to_owned(),
    };
    assert_eq!(
        fetches_after(basic.clone(), Some("u"), "Basic dTpzM2NyM3QtdmFsdWU="),
        2
    );
    assert_eq!(fetches_after(basic.clone(), Some("u"), "Basic bogus"), 1);
    assert_eq!(fetches_after(basic, Some("u"), "token s3cr3t-value"), 1);
}

#[test]
fn evict_if_current_ignores_rules_the_filter_does_not_own() {
    let (fetch, calls) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None, &clock());
    secret_of(&cap, "r").unwrap();
    cap.evict_if_current("r", "token s3cr3t-value");
    secret_of(&cap, "r").unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn base64_matches_known_vectors() {
    for (raw, want) in [
        ("", ""),
        ("M", "TQ=="),
        ("Ma", "TWE="),
        ("Man", "TWFu"),
        ("u:", "dTo="),
    ] {
        assert_eq!(super::base64(raw.as_bytes()), want);
    }
}
