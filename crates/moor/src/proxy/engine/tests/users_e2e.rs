//! Two users' Boxes on one store: each engine injects only its own user's secret, read from the
//! vault on every request, and a deleted secret fails the next request.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::{Identity, SecretName, UserId};
use ureq::Agent;

use super::{Host, Running, Wiring};
use crate::proxy::ledger::tests::{box_ledger_with, OnLedger};
use crate::proxy::nono::capture::UserSecretsFn;
use crate::proxy::nono::tests::upstream::{self, Upstream};
use crate::store::STORE_FILE;
use crate::testing::cx::put_secret;
use crate::vault::Vault;

/// Each user's token; neither may appear anywhere but in the upstream request.
const ANN: &str = "ann-token-0f3c";
const BOB: &str = "bob-token-9a71";

/// The `claude` rule pointed at the fake upstream on `port`, allowing `GET /ok`.
fn claude_rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "claude".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "claude".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// One state dir holding two users, Ann and Bob, each with a `claude` secret and a running Box.
struct Two {
    host: Host,
    up: Upstream,
    ann: (UserId, Running),
    bob: (UserId, Running),
}

/// Starts the engine of a Box named `name` for `user`, reading secrets through its own ledger
/// as the egress Proxy does.
fn start_for(host: &Host, up: &Upstream, vault: &Arc<Vault>, name: &str, user: UserId) -> Running {
    let id = host.new_box(name, Some(user));
    let ledger: OnLedger = box_ledger_with(&host.store, id, Arc::clone(vault));
    let shared = Arc::clone(&ledger);
    let secrets: UserSecretsFn =
        Arc::new(move |name| shared.secret(name).map_err(|e| e.to_string()));
    let spec = ProxySpec {
        box_id: id,
        box_handle: None,
        other_repos: Vec::new(),
        box_repo: None,
        rules: vec![claude_rule(up.port)],
        user,
    };
    let wiring = Wiring {
        secrets,
        upstream_ca: Some(up.ca_path.clone()),
        upstream_dial: None,
        guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, up.port))],
    };
    Running::start(host, id, ledger, &spec, &wiring)
}

fn two() -> Two {
    let host = Host::new();
    let up = upstream::start(host.dir.path());
    let vault = Arc::new(Vault::load_or_create(&host.dir.path().join("vault.key")).unwrap());
    let [ann, bob] = [("ann", ANN, 1), ("bob", BOB, 2)].map(|(name, token, uid)| {
        let (user, _) = host.store.enroll(&Identity::local(uid), None).unwrap();
        put_secret(&host.store, &vault, user, "claude", token);
        (user, start_for(&host, &up, &vault, name, user))
    });
    Two { host, up, ann, bob }
}

impl Two {
    /// The `Authorization` the upstream saw on its last request.
    fn last_auth(&self) -> Option<String> {
        self.up.seen().last().and_then(|s| s.authorization.clone())
    }

    /// Every file under the state dir that a secret must never reach: the Proxy logs and the store.
    fn files(&self) -> Vec<PathBuf> {
        let dir = self.host.dir.path();
        let logs = std::fs::read_dir(dir.join("proxy")).unwrap();
        let mut out: Vec<PathBuf> = logs.map(|e| e.unwrap().path()).collect();
        out.retain(|p| p.is_file());
        out.extend(["", "-wal", "-shm"].map(|s| dir.join(format!("{STORE_FILE}{s}"))));
        out
    }
}

/// GETs `/ok` on the upstream; status or error text.
fn get(agent: &Agent, up: &Upstream) -> Result<u16, String> {
    agent
        .get(format!("https://localhost:{}/ok", up.port))
        .header("Authorization", "Bearer placeholder")
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}

/// Asserts the upstream's last request carried `token`.
fn assert_last(t: &Two, token: &str) {
    let want = format!("Bearer {token}");
    assert_eq!(t.last_auth().as_deref(), Some(want.as_str()));
}

/// Deletes Ann's secret; her next request fails before upstream and is recorded.
fn logout_ann(t: &Two, ann: &Agent) {
    let claude = SecretName::parse("claude").unwrap();
    assert!(t.host.store.delete_user_secret(t.ann.0, &claude).unwrap());
    let seen = t.up.seen().len();
    assert_ne!(get(ann, &t.up), Ok(200), "a deleted secret still worked");
    assert_eq!(
        t.up.seen().len(),
        seen,
        "the failed request reached upstream"
    );
    t.ann
        .1
        .events_until("the credential failure event", |events| {
            events.iter().any(|(k, _)| k == "proxy_credential_failed")
        });
}

/// Asserts neither token nor the vault key is in the specs, the logs, the events or the store.
fn assert_nowhere(t: &Two) {
    let mut said = Vec::new();
    for path in t.files() {
        said.extend(std::fs::read(&path).unwrap_or_default());
    }
    for run in [&t.ann.1, &t.bob.1] {
        said.extend(format!("{:?}", run.events()).into_bytes());
    }
    let key = std::fs::read_to_string(t.host.dir.path().join("vault.key")).unwrap();
    for secret in [ANN, BOB, key.trim()] {
        let found = said.windows(secret.len()).any(|w| w == secret.as_bytes());
        assert!(!found, "a secret reached a spec, log, event or the store");
    }
}

#[test]
fn each_users_box_injects_only_that_users_secret_and_a_deleted_one_fails_next() {
    let t = two();
    let (ann, bob) = (t.ann.1.client(), t.bob.1.client());
    assert_eq!(get(&ann, &t.up), Ok(200));
    assert_last(&t, ANN);
    assert_eq!(get(&bob, &t.up), Ok(200));
    assert_last(&t, BOB);
    logout_ann(&t, &ann);
    assert_eq!(get(&bob, &t.up), Ok(200), "Bob's Box kept working");
    assert_last(&t, BOB);
    t.ann.1.drain();
    let failures = t.ann.1.events();
    let failures = failures
        .iter()
        .filter(|(k, _)| k == "proxy_credential_failed");
    assert_eq!(
        failures.count(),
        1,
        "one failed request records one credential failure"
    );
    assert_nowhere(&t);
}

#[test]
fn a_box_keeps_injecting_its_users_last_secret_while_the_controller_is_down() {
    let t = two();
    let ann = t.ann.1.client();
    assert_eq!(get(&ann, &t.up), Ok(200));
    t.ann.1.ledger.down();
    assert_eq!(
        get(&ann, &t.up),
        Ok(200),
        "the controller gap failed a request"
    );
    assert_last(&t, ANN);
}
