//! A remote front's lease on the shared Proxy: it spends its user's secret through that front's
//! rules, only while the store holds it, and never reaches a Box's proxy or the machine service.
#![allow(clippy::indexing_slicing, reason = "the base64 digit is below 64")]

use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};

use super::*;

/// A front that runs elsewhere, whose Proxy is the shared one.
pub(super) const REMOTE: &str = "slack";
/// A front beside the controller, with a Proxy of its own.
pub(super) const BESIDE: &str = "web";

/// A signed lease of `front` for the World's user, recorded in the store.
fn leased(w: &World, front: &str) -> String {
    let token = token::mint_lease(EGRESS_KEY.as_bytes()).unwrap();
    w.store
        .add_front_lease(token.as_bytes(), front, user(w), Ts::from_millis(i64::MAX))
        .unwrap();
    token
}

/// The World's user.
fn user(w: &World) -> UserId {
    w.store.user_of(&Identity::local(1)).unwrap().unwrap()
}

/// The CA the remote front trusts.
fn front_ca(w: &World) -> String {
    crate::proxy::fronts::ca_pair(w.dir.path())
        .unwrap()
        .cert_pem
        .clone()
}

/// `text` in standard base64 with padding.
fn base64(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let group = chunk
            .iter()
            .fold(0_u32, |acc, b| (acc << 8) | u32::from(*b))
            << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                let digit = usize::try_from((group >> (18 - 6 * i)) & 63).unwrap();
                out.push(char::from(ALPHABET[digit]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Opens a tunnel to `target` as a remote front's forwarder does, the lease as `Basic`.
fn open_basic(server: &Server, lease: &str, target: &str) -> Result<WebSocket<TcpStream>, u16> {
    let url = format!("ws://{}{PATH}", addr(server));
    let mut req = url.into_client_request().unwrap();
    let basic = base64(&format!("lease:{lease}"));
    let headers = req.headers_mut();
    headers.insert("authorization", format!("Basic {basic}").parse().unwrap());
    headers.insert(TARGET, target.parse().unwrap());
    let stream = TcpStream::connect(addr(server)).map_err(|_| 0_u16)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    match tungstenite::client(req, stream) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))) => {
            Err(resp.status().as_u16())
        }
        Err(e) => unreachable!("handshake failed: {e}"),
    }
}

#[test]
fn a_lease_spends_its_users_secret_through_the_fronts_rules() {
    let w = World::new();
    let server = w.server(10.0);
    let lease = leased(&w, REMOTE);
    let tunnel = open_basic(&server, &lease, &w.target()).unwrap();
    assert_eq!(w.get_trusting(tunnel, &front_ca(&w)), "HTTP/1.1 200 OK");
    let seen = w.up.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.authorization.as_deref(), Some("Bearer test-token-1"));
    assert_eq!(server.drain(Duration::from_secs(5)), 0);
}

#[test]
fn a_lease_the_store_does_not_hold_is_refused() {
    let w = World::new();
    let server = w.server(10.0);
    let lease = leased(&w, REMOTE);
    assert!(w
        .store
        .delete_front_lease(lease.as_bytes(), REMOTE, user(&w))
        .unwrap());
    assert_eq!(open_basic(&server, &lease, &w.target()).err(), Some(401));
    let never = token::mint_lease(EGRESS_KEY.as_bytes()).unwrap();
    assert_eq!(open_basic(&server, &never, &w.target()).err(), Some(401));
    server.drain(Duration::ZERO);
}

#[test]
fn a_lease_under_another_key_is_refused_before_any_lookup() {
    let w = World::new();
    let server = w.server(10.0);
    let forged = token::mint_lease(b"another key").unwrap();
    w.store
        .add_front_lease(
            forged.as_bytes(),
            REMOTE,
            user(&w),
            Ts::from_millis(i64::MAX),
        )
        .unwrap();
    assert_eq!(open_basic(&server, &forged, &w.target()).err(), Some(401));
    server.drain(Duration::ZERO);
}

#[test]
fn a_lease_of_a_front_beside_the_controller_is_not_spent_here() {
    let w = World::new();
    let server = w.server(10.0);
    let lease = leased(&w, BESIDE);
    assert_eq!(open_basic(&server, &lease, &w.target()).err(), Some(401));
    server.drain(Duration::ZERO);
}

#[test]
fn a_lease_never_reaches_the_machine_service() {
    let w = World::new();
    let route = crate::proxy::egress::MachineRoute::parse("machine.test=127.0.0.1:9").unwrap();
    let server = Server::start(Config {
        machine: Some(route),
        ..w.config(10.0, 10.0, 64)
    })
    .unwrap();
    let lease = leased(&w, REMOTE);
    assert_eq!(
        open_basic(&server, &lease, "machine.test:80").err(),
        Some(403)
    );
    server.drain(Duration::ZERO);
}

#[test]
fn a_box_token_is_not_a_lease_and_a_lease_is_not_a_box_token() {
    let w = World::new();
    let server = w.server(10.0);
    let lease = leased(&w, REMOTE);
    assert!(open(&server, &w.token, &w.target()).is_ok());
    assert!(!token::is_lease(w.token.as_bytes()));
    assert!(token::is_lease(lease.as_bytes()));
    server.drain(Duration::ZERO);
}

#[test]
fn a_turn_keeps_its_lease_and_secret_while_the_controller_is_away() {
    let mut w = World::new();
    let server = w.server(10.0);
    let lease = leased(&w, REMOTE);
    let ca = front_ca(&w);
    let tunnel = open_basic(&server, &lease, &w.target()).unwrap();
    assert_eq!(w.get_trusting(tunnel, &ca), "HTTP/1.1 200 OK");
    drop(w.ledger.take());
    let tunnel = open_basic(&server, &lease, &w.target()).unwrap();
    assert_eq!(w.get_trusting(tunnel, &ca), "HTTP/1.1 200 OK");
    let never = token::mint_lease(EGRESS_KEY.as_bytes()).unwrap();
    assert_eq!(open_basic(&server, &never, &w.target()).err(), Some(503));
    server.drain(Duration::ZERO);
}
