//! Lets a real `moor proxy serve` reach a local TLS upstream in tests.
//!
//! The opening is one extra trusted CA and one loopback address the egress guard lets through.
//! Only the `test-upstream` feature compiles it, and `make lint` fails if a release build
//! enables it.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::{Identity, SecretName, UserId};
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId, Secret};

use crate::adapters::clock::SystemClock;
use crate::adapters::rng::SystemRng;
use crate::env::Env;
use crate::proxy::supervisor::LEASE;
use crate::store::{Store, StoreError, STORE_FILE};
use crate::vault::Vault;

/// The PEM file of the one extra CA a Box Proxy trusts upstream.
pub const UPSTREAM_CA: &str = "MOOR_TEST_UPSTREAM_CA";

/// The loopback address a Box Proxy may dial despite the egress guard.
pub const UPSTREAM: &str = "MOOR_TEST_UPSTREAM";

/// The name of the secret [`seed_box`] gives the Box user.
pub const CREDENTIAL: &str = "claude";

/// The extra upstream CA named by [`UPSTREAM_CA`].
pub(crate) fn upstream_ca(env: &Env) -> Option<PathBuf> {
    env.get(UPSTREAM_CA).map(PathBuf::from)
}

/// The one address [`UPSTREAM`] names, only when it is loopback.
pub(crate) fn guard_exempt(env: &Env) -> Vec<SocketAddr> {
    env.get(UPSTREAM)
        .and_then(|a| a.parse::<SocketAddr>().ok())
        .filter(|a| a.ip().is_loopback())
        .into_iter()
        .collect()
}

/// Writes a ready Box `harness` into a fresh state dir; returns its id.
///
/// The Box runs on machine `p/harness` and is owned by local uid 1, whose `claude` secret is
/// `token`. Its Proxy row has one rule, letting `GET /ok` reach `localhost:<upstream_port>`.
/// `vault_key` is the controller's, beside its config. The controller starts the Proxy from that
/// row once it reconciles.
///
/// # Errors
/// Why the store or the vault could not be written.
pub fn seed_box(
    state_dir: &Path,
    vault_key: &Path,
    upstream_port: u16,
    token: &str,
) -> Result<i64, String> {
    let clock = SystemClock::from_env(&Env::from_pairs::<_, String, String>([]))
        .map_err(|e| e.to_string())?;
    let store =
        Store::open(&state_dir.join(STORE_FILE), Arc::new(clock)).map_err(|e| e.to_string())?;
    let vault = Vault::load_or_create(vault_key).map_err(|e| e.to_string())?;
    let (user, _) = store
        .enroll(&Identity::local(1), None)
        .map_err(|e| e.to_string())?;
    let name = SecretName::parse(CREDENTIAL)?;
    let sealed = vault
        .seal(user, &name, &Secret::new(token.as_bytes().to_vec()))
        .map_err(|e| e.to_string())?;
    store
        .set_user_secret(user, &name, &sealed)
        .map_err(|e| e.to_string())?;
    let handle = BoxHandle::new("p/harness");
    let id = ready_box(&store, user, &handle).map_err(|e| e.to_string())?;
    drop(
        store
            .reserve(id, &SystemRng, LEASE)
            .map_err(|e| e.to_string())?,
    );
    let spec = ProxySpec {
        box_id: id,
        box_handle: Some(handle),
        box_repo: None,
        other_repos: Vec::new(),
        rules: vec![rule(upstream_port)],
        user,
    };
    store.set_proxy_spec(id, &spec).map_err(|e| e.to_string())?;
    Ok(id.get())
}

/// Inserts Box `harness` for `user` and takes it to Ready on `handle`.
fn ready_box(store: &Store, user: UserId, handle: &BoxHandle) -> Result<BoxId, StoreError> {
    let new = NewBox {
        name: BoxName::parse_allow_bare("harness")
            .map_err(|e| StoreError::Invalid(e.to_string()))?,
        repo: None,
    };
    let id = store.apply(Decision::insert(new).requester(user))?.box_id;
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admitted)?;
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: handle.clone(),
        });
    store.apply(ready)?;
    Ok(id)
}

/// The one rule: `GET /ok` on `localhost:<port>`, with the `claude` secret as a bearer token.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: CREDENTIAL.to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: CREDENTIAL.to_owned(),
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
