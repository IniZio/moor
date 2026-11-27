//! A Box's spec change: the proxy is rebuilt for new tunnels while a tunnel already open keeps
//! the old one, which stops once that tunnel ends.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use moor_core::config::proxy::ResolvedInject;
use moor_core::seam::clock::{Clock, Ts};

use super::{eventually, open, Server, World};
use crate::proxy::ledger::Endpoint;
use crate::testing::clock::FakeClock;

/// Counts, from now on, the panics on the shared Proxy's runtime threads, where a panic in a
/// tunnel's task is caught and never fails a test by itself.
fn egress_panics() -> Arc<AtomicUsize> {
    let seen = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&seen);
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("moor-egress") {
            count.fetch_add(1, Ordering::SeqCst);
        }
        before(info);
    }));
    seen
}

/// The spec of `w`'s Box with its credential sent as `Token {secret}`.
fn token_form(w: &World) {
    let mut spec = w.store.proxy_spec(w.id).unwrap().unwrap();
    for r in &mut spec.rules {
        r.inject = ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Token {secret}".to_owned(),
        };
    }
    w.store.set_proxy_spec(w.id, &spec).unwrap();
}

/// The authorization the upstream saw last.
fn last_auth(w: &World) -> Option<String> {
    w.up.seen().last().and_then(|s| s.authorization.clone())
}

/// How long a held tend waits, which only [`Held::release`] ends.
pub(super) const HOLD: Duration = Duration::from_secs(3600);

/// A fake clock whose sleeps of [`HOLD`] block until released, so the Proxy never tends.
pub(super) struct Held {
    fake: FakeClock,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Held {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            fake: FakeClock::new(Ts::from_millis(1_000)),
            released: Mutex::new(false),
            wake: Condvar::new(),
        })
    }

    /// Lets every held sleep end, so the Proxy can drain.
    pub(super) fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

impl Clock for Held {
    fn now(&self) -> Ts {
        self.fake.now()
    }

    fn instant(&self) -> Instant {
        self.fake.instant()
    }

    fn sleep(&self, d: Duration) {
        if d == HOLD {
            let released = self.released.lock().unwrap();
            drop(self.wake.wait_while(released, |r| !*r).unwrap());
        }
        self.fake.sleep(d);
    }
}

/// The shared Proxy of `w` with tending held by `clock`.
fn untended(w: &World, clock: &Arc<Held>) -> Server {
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.tend = HOLD;
    cfg.warm = None;
    cfg.clock = Arc::clone(clock) as Arc<dyn Clock>;
    Server::start(cfg).unwrap()
}

#[test]
fn a_new_tunnel_after_a_spec_change_uses_the_new_spec_before_any_tend() {
    let w = World::new();
    let clock = Held::new();
    let server = untended(&w, &clock);
    let held = open(&server, &w.token, &w.target()).unwrap();
    token_form(&w);
    let fresh = open(&server, &w.token, &w.target()).unwrap();
    assert_eq!(w.get(fresh), "HTTP/1.1 200 OK");
    assert_eq!(
        last_auth(&w).as_deref(),
        Some("Token test-token-1"),
        "the first tunnel after the change goes through the new spec"
    );
    assert_eq!(w.get(held), "HTTP/1.1 200 OK");
    assert_eq!(
        last_auth(&w).as_deref(),
        Some("Bearer test-token-1"),
        "the tunnel opened before the change keeps the old spec"
    );
    clock.release();
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_spec_rebuild_on_admit_does_not_panic() {
    let panics = egress_panics();
    let w = World::new();
    let clock = Held::new();
    let server = untended(&w, &clock);
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    let old = server.engines();
    token_form(&w);
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(last_auth(&w).as_deref(), Some("Token test-token-1"));
    eventually("the replaced proxy, which no tunnel holds, to stop", || {
        old.iter().all(|a| TcpStream::connect(a).is_err())
    });
    assert_eq!(panics.load(Ordering::SeqCst), 0, "no panic on the rebuild");
    clock.release();
    server.drain(Duration::from_secs(5));
}

/// Relays the ledger at `real` on `socket` as a controller from before `token_spec` does:
/// that call is refused as one it cannot read. Counts the refusals.
fn older_controller(socket: &Path, real: &Path) -> Arc<AtomicUsize> {
    let refused = Arc::new(AtomicUsize::new(0));
    let listener = UnixListener::bind(socket).unwrap();
    let (count, real) = (Arc::clone(&refused), real.to_owned());
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let (count, real) = (Arc::clone(&count), real.clone());
            std::thread::spawn(move || relay(&client, &UnixStream::connect(real).unwrap(), &count));
        }
    });
    refused
}

/// Answers each line from `client` through `controller`, refusing `token_spec`.
fn relay(client: &UnixStream, controller: &UnixStream, refused: &AtomicUsize) {
    let (mut from, mut back) = (BufReader::new(client), BufReader::new(controller));
    let mut line = String::new();
    while from.read_line(&mut line).is_ok_and(|n| n > 0) {
        let reply = if line.starts_with(r#"{"token_spec""#) {
            refused.fetch_add(1, Ordering::SeqCst);
            "{\"Err\":{\"refused\":\"bad call: unknown variant\"}}\n".to_owned()
        } else {
            back.get_mut().write_all(line.as_bytes()).unwrap();
            let mut reply = String::new();
            back.read_line(&mut reply).unwrap();
            reply
        };
        if from.get_mut().write_all(reply.as_bytes()).is_err() {
            return;
        }
        line.clear();
    }
}

#[test]
fn an_older_controller_refusing_token_spec_keeps_tend_behaviour() {
    let w = World::new();
    let old = w.dir.path().join("old.sock");
    let refused = older_controller(&old, &w.dir.path().join("proxy.sock"));
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.ledger = Endpoint::Unix(old);
    let server = Server::start(cfg).unwrap();
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(last_auth(&w).as_deref(), Some("Bearer test-token-1"));
    assert!(refused.load(Ordering::SeqCst) > 0, "it asked for the spec");
    let before = server.engines();
    token_form(&w);
    eventually("tending to rebuild the proxy", || {
        server.engines() != before
    });
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    assert_eq!(last_auth(&w).as_deref(), Some("Token test-token-1"));
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_tunnel_open_across_a_spec_change_keeps_the_old_proxy_until_it_ends() {
    let panics = egress_panics();
    let w = World::new();
    let server = w.server(10.0);
    let held = open(&server, &w.token, &w.target()).unwrap();
    let old = server.engines();
    let mut spec = w.store.proxy_spec(w.id).unwrap().unwrap();
    for r in &mut spec.rules {
        r.inject = ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Token {secret}".to_owned(),
        };
    }
    w.store.set_proxy_spec(w.id, &spec).unwrap();
    eventually("the new spec's credential form", || {
        let tunnel = open(&server, &w.token, &w.target()).unwrap();
        drop(w.get(tunnel));
        w.up.seen()
            .last()
            .and_then(|s| s.authorization.clone())
            .as_deref()
            == Some("Token test-token-1")
    });
    assert_eq!(w.get(held), "HTTP/1.1 200 OK");
    assert_eq!(
        w.up.seen()
            .last()
            .and_then(|s| s.authorization.clone())
            .as_deref(),
        Some("Bearer test-token-1"),
        "the tunnel opened before the change goes through the old proxy"
    );
    eventually("the old proxy to stop once its last tunnel ends", || {
        old.iter().all(|a| TcpStream::connect(a).is_err())
    });
    assert_eq!(
        panics.load(Ordering::SeqCst),
        0,
        "the old proxy stopped without a panic"
    );
    server.drain(Duration::from_secs(5));
}

/// Holds each gated call relayed while armed, as a slow controller would, and counts the held ones.
#[derive(Default)]
pub(super) struct CaGate {
    /// Whether `ca` calls are held, and how many are.
    state: Mutex<(bool, usize)>,
    wake: Condvar,
}

impl CaGate {
    pub(super) fn set(&self, armed: bool) {
        self.state.lock().unwrap().0 = armed;
        self.wake.notify_all();
    }

    pub(super) fn held(&self) -> usize {
        self.state.lock().unwrap().1
    }

    /// Waits while armed, counted as held.
    fn pass(&self) {
        let mut s = self.state.lock().unwrap();
        s.1 += 1;
        self.wake.notify_all();
        s = self.wake.wait_while(s, |s| s.0).unwrap();
        s.1 -= 1;
    }
}

/// Relays the ledger at `real` on `socket`, passing each call whose line starts with `call`
/// through the gate first.
pub(super) fn gated_controller(socket: &Path, real: &Path, call: &'static str) -> Arc<CaGate> {
    let gate = Arc::new(CaGate::default());
    let listener = UnixListener::bind(socket).unwrap();
    let (each, real) = (Arc::clone(&gate), real.to_owned());
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let (gate, real) = (Arc::clone(&each), real.clone());
            std::thread::spawn(move || {
                let controller = UnixStream::connect(real).unwrap();
                let (mut from, mut back) = (BufReader::new(&client), BufReader::new(&controller));
                let mut line = String::new();
                while from.read_line(&mut line).is_ok_and(|n| n > 0) {
                    if line.starts_with(call) {
                        gate.pass();
                    }
                    back.get_mut().write_all(line.as_bytes()).unwrap();
                    let mut reply = String::new();
                    back.read_line(&mut reply).unwrap();
                    if from.get_mut().write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                    line.clear();
                }
            });
        }
    });
    gate
}

#[test]
fn a_tunnel_and_tending_never_build_a_proxy_for_one_box_at_once() {
    let w = World::new();
    let gated = w.dir.path().join("gated.sock");
    let gate = gated_controller(&gated, &w.dir.path().join("proxy.sock"), r#""ca""#);
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.ledger = Endpoint::Unix(gated);
    let server = Server::start(cfg).unwrap();
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    gate.set(true);
    token_form(&w);
    std::thread::scope(|s| {
        let tunnel = s.spawn(|| w.get(open(&server, &w.token, &w.target()).unwrap()));
        eventually("a rebuild to read the CA", || gate.held() > 0);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            gate.held(),
            1,
            "a second build of the Box's proxy shares its CA directory with the first"
        );
        gate.set(false);
        assert_eq!(tunnel.join().unwrap(), "HTTP/1.1 200 OK");
    });
    assert_eq!(last_auth(&w).as_deref(), Some("Token test-token-1"));
    server.drain(Duration::from_secs(5));
}
