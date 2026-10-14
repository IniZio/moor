//! Credential resolver behaviour: routing, caching and error text.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::proxy_gate::{Inject, Plan, RouteId};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo};
use moor_core::Secret;

use super::{ResolveError, Resolver, FORGE_SOURCE, MODEL_SOURCE, TOKEN_TTL};
use crate::adapters::clock::SystemClock;
use crate::proxy::head::Head;
use crate::proxy::relay::Relay;
use crate::proxy::server::{body_cap, Forward, ForwardHandler};
use crate::proxy::upstream::{RouteCredential, Upstreams};
use crate::testing::clock::FakeClock;
use crate::testing::forge::{FakeForge, ForgeCall};

/// Read deadline so a failing test turns red instead of hanging.
const WAIT: Duration = Duration::from_secs(10);

fn repo() -> ForgeRepo {
    ForgeRepo::new("acme", "widgets")
}

fn route(name: &str) -> RouteId {
    RouteId::new(name)
}

/// Model source returning `token`, plus the number of times it ran.
fn model_ok(token: &'static str) -> (super::ModelToken, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let source: super::ModelToken = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(Secret::new(token.as_bytes().to_vec()))
    });
    (source, calls)
}

/// Model source that fails on its first call and succeeds afterwards.
fn model_flaky() -> (super::ModelToken, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let source: super::ModelToken = Arc::new(move || {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("login expired".to_owned())
        } else {
            Ok(Secret::new(b"model-tok".to_vec()))
        }
    });
    (source, calls)
}

/// Model source that always fails with `reason`.
fn model_err(reason: &'static str) -> super::ModelToken {
    Arc::new(move || Err(reason.to_owned()))
}

/// Forge that fails on its first token request and succeeds afterwards.
#[derive(Default)]
struct FlakyForge {
    calls: Mutex<usize>,
}

impl Forge for FlakyForge {
    fn routes(&self) -> Vec<moor_core::proxy_gate::RouteRow> {
        Vec::new()
    }

    fn repo_of(&self, _origin: &str) -> Option<ForgeRepo> {
        None
    }

    fn repo_token(&self, _repo: &ForgeRepo) -> Result<Secret, ForgeError> {
        let mut n = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        *n += 1;
        if *n == 1 {
            Err(ForgeError::Network("unreachable".to_owned()))
        } else {
            Ok(Secret::new(b"forge-tok".to_vec()))
        }
    }
}

/// A resolver over `forge` for the Task's repo, with a fake clock.
fn resolver_with(
    model: super::ModelToken,
    forge: Arc<dyn Forge>,
    with_repo: bool,
) -> (Resolver, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let resolver = Resolver::new(
        model,
        forge,
        with_repo.then(repo),
        Arc::clone(&clock) as Arc<dyn Clock>,
    );
    (resolver, clock)
}

/// Forge scripted to hand out `tok` for the Task's repo.
fn forge_with(tok: &[u8]) -> Arc<FakeForge> {
    Arc::new(FakeForge::new().with_token(repo(), tok))
}

fn token_calls(forge: &FakeForge) -> usize {
    forge
        .calls()
        .iter()
        .filter(|c| matches!(c, ForgeCall::RepoToken(_)))
        .count()
}

/// The model route yields the model token; both forge routes yield the repo token.
#[test]
fn routes_resolve_to_their_tokens() {
    let (model, _) = model_ok("model-tok");
    let forge = forge_with(b"forge-tok");
    let (r, _) = resolver_with(model, forge.clone(), true);
    assert_eq!(r.resolve(&route("model")).unwrap().expose(), b"model-tok");
    assert_eq!(
        r.resolve(&route("forge.git")).unwrap().expose(),
        b"forge-tok"
    );
    assert_eq!(
        r.resolve(&route("forge.api")).unwrap().expose(),
        b"forge-tok"
    );
    assert_eq!(forge.calls(), vec![ForgeCall::RepoToken(repo())]);
}

/// A route that is neither model nor forge is rejected.
#[test]
fn unknown_route_is_rejected() {
    let (model, calls) = model_ok("m");
    let forge = forge_with(b"f");
    let (r, _) = resolver_with(model, forge.clone(), true);
    let err = r.resolve(&route("other")).err().unwrap();
    assert!(
        matches!(&err, ResolveError::UnknownRoute(n) if n == "other"),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "no credential for route other");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(forge.calls().is_empty());
}

/// A forge route on a Task without a repo fails without calling the forge.
#[test]
fn forge_route_without_repo_fails_before_calling_forge() {
    let (model, _) = model_ok("m");
    let forge = forge_with(b"f");
    let (r, _) = resolver_with(model, forge.clone(), false);
    for name in ["forge.git", "forge.api"] {
        let err = r.resolve(&route(name)).err().unwrap();
        assert!(matches!(err, ResolveError::NoRepo), "{err:?}");
        assert_eq!(
            err.to_string(),
            format!("{FORGE_SOURCE}: the Task has no forge repo")
        );
    }
    assert!(forge.calls().is_empty());
}

/// A second forge call inside the TTL is served from the cache.
#[test]
fn forge_token_is_cached_within_ttl() {
    let (model, _) = model_ok("m");
    let forge = forge_with(b"forge-tok");
    let (r, _) = resolver_with(model, forge.clone(), true);
    r.resolve(&route("forge.git")).unwrap();
    let again = r.resolve(&route("forge.git")).unwrap();
    assert_eq!(again.expose(), b"forge-tok");
    assert_eq!(forge.calls(), vec![ForgeCall::RepoToken(repo())]);
}

/// forge.git and forge.api share one cache slot.
#[test]
fn forge_routes_share_one_cache_slot() {
    let (model, _) = model_ok("m");
    let forge = forge_with(b"forge-tok");
    let (r, _) = resolver_with(model, forge.clone(), true);
    r.resolve(&route("forge.git")).unwrap();
    r.resolve(&route("forge.api")).unwrap();
    assert_eq!(token_calls(&forge), 1);
}

/// The forge is asked again once the TTL has elapsed.
#[test]
fn forge_token_is_refetched_after_ttl() {
    let (model, _) = model_ok("m");
    let forge = forge_with(b"forge-tok");
    let (r, clock) = resolver_with(model, forge.clone(), true);
    r.resolve(&route("forge.git")).unwrap();
    clock.sleep(TOKEN_TTL);
    r.resolve(&route("forge.git")).unwrap();
    assert_eq!(token_calls(&forge), 2);
}

/// The model source is cached inside the TTL and refetched after it.
#[test]
fn model_token_is_cached_until_ttl() {
    let (model, calls) = model_ok("model-tok");
    let (r, clock) = resolver_with(model, forge_with(b"f"), true);
    r.resolve(&route("model")).unwrap();
    r.resolve(&route("model")).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    clock.sleep(TOKEN_TTL);
    r.resolve(&route("model")).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// The model cache and the forge cache are independent.
#[test]
fn model_and_forge_caches_are_independent() {
    let (model, calls) = model_ok("model-tok");
    let forge = forge_with(b"forge-tok");
    let (r, _) = resolver_with(model, forge.clone(), true);
    r.resolve(&route("model")).unwrap();
    r.resolve(&route("forge.git")).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(token_calls(&forge), 1);
}

/// A failed model lookup is not cached.
#[test]
fn model_failure_is_not_cached() {
    let (model, calls) = model_flaky();
    let (r, _) = resolver_with(model, forge_with(b"f"), true);
    assert!(r.resolve(&route("model")).is_err());
    assert_eq!(r.resolve(&route("model")).unwrap().expose(), b"model-tok");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// A failed forge lookup is not cached.
#[test]
fn forge_failure_is_not_cached() {
    let (model, _) = model_ok("m");
    let forge = Arc::new(FlakyForge::default());
    let (r, _) = resolver_with(model, forge.clone(), true);
    assert!(r.resolve(&route("forge.git")).is_err());
    assert_eq!(
        r.resolve(&route("forge.api")).unwrap().expose(),
        b"forge-tok"
    );
    assert_eq!(*forge.calls.lock().unwrap(), 2);
}

/// A model error names its source and the reason.
#[test]
fn model_error_names_source() {
    let (r, _) = resolver_with(model_err("login expired"), forge_with(b"f"), true);
    let err = r.resolve(&route("model")).err().unwrap();
    assert_eq!(err.to_string(), format!("{MODEL_SOURCE}: login expired"));
}

/// A forge error names its source and carries no token.
#[test]
fn forge_error_names_source_without_token() {
    let (model, _) = model_ok("sentinel-4b1d");
    let forge =
        Arc::new(FakeForge::new().with_token_error(repo(), ForgeError::Auth("denied".into())));
    let (r, _) = resolver_with(model, forge, true);
    r.resolve(&route("model")).unwrap();
    let text = r.resolve(&route("forge.git")).err().unwrap().to_string();
    assert!(text.starts_with(FORGE_SOURCE), "{text}");
    assert!(text.contains("denied"), "{text}");
    assert!(!text.contains("sentinel-4b1d"), "{text}");
}

/// Debug output never shows cached token bytes.
#[test]
fn debug_output_hides_tokens() {
    let (model, _) = model_ok("sentinel-4b1d");
    let forge = forge_with(b"sentinel-7e2a");
    let (r, _) = resolver_with(model, forge, true);
    r.resolve(&route("model")).unwrap();
    r.resolve(&route("forge.git")).unwrap();
    let dump = format!("{r:?}");
    assert!(
        !dump.contains("sentinel-4b1d") && !dump.contains("sentinel-7e2a"),
        "{dump}"
    );
}

/// The trait method maps any resolve failure to the opaque error.
#[test]
fn route_credential_maps_errors_to_cred_error() {
    let (r, _) = resolver_with(model_err("x"), forge_with(b"f"), true);
    assert!(r.credential(&route("model")).is_err());
    assert_eq!(r.credential(&route("forge.git")).unwrap().expose(), b"f");
}

/// A failing model source makes the relay answer 502 and never dial upstream.
#[test]
fn relay_answers_502_when_model_source_fails() {
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    upstream.set_nonblocking(true).unwrap();
    let port = upstream.local_addr().unwrap().port();
    let ups = Upstreams::new(
        [("model".to_owned(), format!("http://127.0.0.1:{port}"))],
        true,
    )
    .unwrap();
    let (r, _) = resolver_with(model_err("login expired"), forge_with(b"f"), true);
    let relay = Relay::new(ups, Arc::new(r), Arc::new(SystemClock::real()), body_cap);
    let req = Forward {
        head: Head {
            method: "GET".to_owned(),
            target: "/ignored".to_owned(),
            headers: Vec::new(),
        },
        plan: Plan {
            route: route("model"),
            method: "GET".to_owned(),
            upstream_path: "/v1/x".to_owned(),
            headers: Vec::new(),
            inject: Inject::Bearer,
        },
        body_prefix: Vec::new(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    let done = thread::spawn(move || relay.forward(req, &mut server));
    let mut out = Vec::new();
    client.read_to_end(&mut out).unwrap();
    let reply = String::from_utf8(out).unwrap();
    assert_eq!(done.join().unwrap(), 502);
    assert!(
        reply.ends_with("\r\n\r\nupstream credential unavailable for route model\n"),
        "{reply}"
    );
    let accepted = upstream.accept();
    assert!(
        matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "{accepted:?}"
    );
}
