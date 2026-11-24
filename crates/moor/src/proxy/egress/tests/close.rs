//! A Box's proxy closing while a tunnel asks for a proxy of the same Box.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Clock;

use super::rebuild::{gated_controller, Held, HOLD};
use super::{eventually, open, Server, World};
use crate::proxy::ledger::Endpoint;
use crate::proxy::nono::ca;

#[test]
fn a_tunnel_while_the_box_proxy_closes_gets_no_new_proxy() {
    let w = World::new();
    let gated = w.dir.path().join("gated.sock");
    let gate = gated_controller(
        &gated,
        &w.dir.path().join("proxy.sock"),
        r#"{"record_event""#,
    );
    let clock = Held::new();
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.ledger = Endpoint::Unix(gated);
    cfg.tend = HOLD;
    cfg.warm = None;
    cfg.clock = Arc::clone(&clock) as Arc<dyn Clock>;
    let server = Server::start(cfg).unwrap();
    assert_eq!(
        w.get(open(&server, &w.token, &w.target()).unwrap()),
        "HTTP/1.1 200 OK"
    );
    // A refused destination leaves an audit event the close delivers.
    drop(open(&server, &w.token, "127.0.0.1:9").map(|t| w.get(t)));
    let ca_dir = ca::ca_dir(&w.dir.path().join("egress"), w.id);
    gate.set(true);
    std::thread::scope(|s| {
        let closing = s.spawn(|| server.close_now(w.id));
        eventually("the close to deliver the Box's audit", || gate.held() > 0);
        let during = open(&server, &w.token, &w.target()).map(|t| w.get(t));
        gate.set(false);
        closing.join().unwrap();
        assert_eq!(
            during,
            Err(503),
            "no proxy is built while the Box's proxy closes"
        );
    });
    assert_eq!(server.serving(), 0, "no proxy of the closed Box runs");
    assert!(!ca_dir.exists(), "the closed Box's CA is gone");
    clock.release();
    server.drain(Duration::from_secs(5));
}
