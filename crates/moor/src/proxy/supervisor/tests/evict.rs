//! Which request heads make the GraphQL filter drop a cached credential.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moor_core::config::proxy::ResolvedInject;
use moor_core::seam::clock::{Clock, Ts};
use moor_core::Secret;
use nono_proxy::capture::{CredentialCaptureBackend as _, CredentialCaptureRequest};

use crate::proxy::graphql::http::Head;
use crate::proxy::nono::capture::{base64, MoorCapture, RuleSource};
use crate::proxy::supervisor::listen::evict_credentialed;
use crate::testing::clock::FakeClock;

/// The credential the capture holds.
const SECRET: &str = "tok-123";

/// A capture for rule `r` that counts fetches, owned by the filter with `inject`.
fn capture(inject: &ResolvedInject, user: Option<&str>) -> (MoorCapture, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let source = RuleSource {
        config_key: "credentials.K".to_owned(),
        fetch: Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Secret::new(SECRET.as_bytes().to_vec()))
        }),
        basic_user: user.map(str::to_owned),
    };
    let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let mut cap = MoorCapture::new(BTreeMap::from([("r".to_owned(), source)]), clock);
    cap.defer_evictions([("r".to_owned(), inject.clone())]);
    (cap, calls)
}

/// Warms the cache.
fn warm(cap: &MoorCapture) {
    let request = CredentialCaptureRequest {
        credential_name: "r".to_owned(),
        route_id: "r__0".to_owned(),
        request_host: "h".to_owned(),
        request_path: "/".to_owned(),
        request_method: "GET".to_owned(),
        session_id: "s".to_owned(),
        cache_scope: "c".to_owned(),
    };
    assert!(cap.capture(request).is_ok());
}

/// A request head with `headers`.
fn head(headers: &[(&str, &str)]) -> Head {
    Head {
        raw: Vec::new(),
        method: "POST".to_owned(),
        target: "/graphql".to_owned(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

/// Fetch count after a 401 for `h` against rule `r`.
fn fetches(inject: &ResolvedInject, header: &str, user: Option<&str>, h: &Head) -> usize {
    let (cap, calls) = capture(inject, user);
    warm(&cap);
    let rules = [("r".to_owned(), header.to_owned(), inject.clone())];
    evict_credentialed(&cap, &rules, h);
    warm(&cap);
    calls.load(Ordering::SeqCst)
}

fn custom() -> ResolvedInject {
    ResolvedInject::Header {
        header: "X-Token".to_owned(),
        form: "{secret}".to_owned(),
    }
}

#[test]
fn spoofed_authorization_does_not_evict() {
    let inject = ResolvedInject::Header {
        header: "authorization".to_owned(),
        form: "token {secret}".to_owned(),
    };
    let spoof = head(&[("Authorization", "token bogus")]);
    assert_eq!(fetches(&inject, "authorization", None, &spoof), 1);
}

#[test]
fn credentialed_401_through_a_custom_header_evicts() {
    let real = head(&[("authorization", "token bogus"), ("x-token", SECRET)]);
    assert_eq!(fetches(&custom(), "X-Token", None, &real), 2);
    let forged = head(&[("x-token", "bogus")]);
    assert_eq!(fetches(&custom(), "X-Token", None, &forged), 1);
}

#[test]
fn credentialed_401_through_basic_auth_evicts() {
    let basic = ResolvedInject::BasicAuth {
        user: "u".to_owned(),
    };
    let value = format!("Basic {}", base64(format!("u:{SECRET}").as_bytes()));
    let real = head(&[("Authorization", &value)]);
    assert_eq!(fetches(&basic, "authorization", Some("u"), &real), 2);
    let forged = head(&[("Authorization", "Basic bogus")]);
    assert_eq!(fetches(&basic, "authorization", Some("u"), &forged), 1);
}
