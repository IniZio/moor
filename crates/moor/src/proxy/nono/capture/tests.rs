//! Capture backend behaviour: a fresh read per request, error text that names only the secret.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::Secret;
use nono_proxy::capture::{
    CredentialCaptureBackend, CredentialCaptureMaterial, CredentialCaptureRequest,
};

use super::{sources_for, FetchFn, MoorCapture, RuleSource, SecretsFn};
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::cx::put_secret;
use crate::vault::Vault;

const SECRET: &str = "s3cr3t-value";

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

fn capture_with(fetch: FetchFn, basic_user: Option<&str>) -> MoorCapture {
    let source = RuleSource {
        config_key: "secret k".to_owned(),
        fetch,
        basic_user: basic_user.map(str::to_owned),
    };
    MoorCapture::new(BTreeMap::from([("r".to_owned(), source)]))
}

fn secret_of(cap: &MoorCapture, rule: &str) -> Result<String, String> {
    cap.capture(request(rule))
        .map(|r| match r.material {
            CredentialCaptureMaterial::Secret(s) => s.to_string(),
            CredentialCaptureMaterial::Headers(_) => String::new(),
        })
        .map_err(|e| e.reason)
}

#[test]
fn every_request_reads_the_source_again() {
    let (fetch, calls) = counting(vec![Ok("one"), Ok("two")]);
    let cap = capture_with(fetch, None);
    assert_eq!(secret_of(&cap, "r").as_deref(), Ok("one"));
    assert_eq!(secret_of(&cap, "r").as_deref(), Ok("two"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_failed_read_names_only_the_secret() {
    let (fetch, _) = counting(vec![Err(SECRET)]);
    let cap = capture_with(fetch, None);
    assert_eq!(secret_of(&cap, "r"), Err("secret k unavailable".to_owned()));
}

#[test]
fn blank_secret_is_rejected() {
    let (fetch, _) = counting(vec![Ok("  \n")]);
    let cap = capture_with(fetch, None);
    assert_eq!(secret_of(&cap, "r"), Err("secret k unavailable".to_owned()));
}

#[test]
fn non_utf8_secret_names_the_key() {
    let fetch: FetchFn = Arc::new(|| Ok(Secret::new(vec![0xff, 0xfe, b'a'])));
    let cap = capture_with(fetch, None);
    assert_eq!(secret_of(&cap, "r"), Err("secret k unavailable".to_owned()));
}

#[test]
fn unknown_rule_errors() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None);
    assert_eq!(
        secret_of(&cap, "nope"),
        Err("unknown proxy rule nope".to_owned())
    );
}

#[test]
fn basic_user_prefixes_the_secret() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, Some("x-access-token"));
    assert_eq!(
        secret_of(&cap, "r").unwrap(),
        format!("x-access-token:{SECRET}")
    );
}

#[test]
fn debug_has_rule_names_only() {
    let (fetch, _) = counting(vec![Ok(SECRET)]);
    let cap = capture_with(fetch, None);
    secret_of(&cap, "r").unwrap();
    let shown = format!("{cap:?}");
    assert!(shown.contains("\"r\""), "{shown}");
    assert!(!shown.contains(SECRET));
}

fn rule(name: &str, credential: &str) -> ResolvedRule {
    ResolvedRule {
        name: name.to_owned(),
        hosts: vec!["example.com".to_owned()],
        credential: credential.to_owned(),
        inject: ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    }
}

/// A real store and vault under a temp dir, with two enrolled users.
struct Vaulted {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    vault: Arc<Vault>,
    ann: UserId,
    bob: UserId,
}

fn vaulted() -> Vaulted {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let ann = store
        .enroll(&Identity::parse("local:1").unwrap(), None)
        .unwrap()
        .0;
    let bob = store
        .enroll(&Identity::parse("local:2").unwrap(), None)
        .unwrap()
        .0;
    Vaulted {
        _dir: dir,
        store: Arc::new(store),
        vault: Arc::new(vault),
        ann,
        bob,
    }
}

impl Vaulted {
    fn secrets(&self) -> SecretsFn {
        let store = Arc::clone(&self.store);
        Arc::new(move |vault, user, name| {
            store
                .user_secret(vault, user, name)
                .map_err(|e| e.to_string())
        })
    }

    fn capture_for(&self, user: UserId) -> MoorCapture {
        let rules = [rule("claude", "claude")];
        MoorCapture::new(sources_for(&rules, &self.secrets(), &self.vault, user))
    }
}

#[test]
fn each_user_gets_only_their_own_secret() {
    let v = vaulted();
    put_secret(&v.store, &v.vault, v.ann, "claude", "ann-token");
    put_secret(&v.store, &v.vault, v.bob, "claude", "bob-token");
    assert_eq!(
        secret_of(&v.capture_for(v.ann), "claude").unwrap(),
        "ann-token"
    );
    assert_eq!(
        secret_of(&v.capture_for(v.bob), "claude").unwrap(),
        "bob-token"
    );
}

#[test]
fn a_deleted_secret_fails_the_next_request() {
    let v = vaulted();
    put_secret(&v.store, &v.vault, v.ann, "claude", "ann-token");
    let cap = v.capture_for(v.ann);
    assert_eq!(secret_of(&cap, "claude").unwrap(), "ann-token");
    let name = moor_core::user::SecretName::parse("claude").unwrap();
    v.store.delete_user_secret(v.ann, &name).unwrap();
    assert_eq!(
        secret_of(&cap, "claude"),
        Err("secret claude unavailable".to_owned())
    );
}

#[test]
fn a_changed_secret_counts_from_the_next_request() {
    let v = vaulted();
    put_secret(&v.store, &v.vault, v.ann, "claude", "old");
    let cap = v.capture_for(v.ann);
    assert_eq!(secret_of(&cap, "claude").unwrap(), "old");
    put_secret(&v.store, &v.vault, v.ann, "claude", "new");
    assert_eq!(secret_of(&cap, "claude").unwrap(), "new");
}
