//! A Box's spec change: the proxy is rebuilt for new tunnels while a tunnel already open keeps
//! the old one, which stops once that tunnel ends.

use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::proxy::ResolvedInject;

use super::{eventually, open, World};

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
