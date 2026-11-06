//! Plain `http://` requests through the shared Proxy: to the machine service with the Box's
//! identity, and to any other host as a CONNECT through the Box's proxy, never with a credential.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_tungstenite::tungstenite::{Message, WebSocket};

use super::{open, open_as, Server, World};
use crate::proxy::egress::MachineRoute;

/// The machine service's host name, as Boxes request it.
const MACHINE: &str = "machine.moor.internal";

/// A plain HTTP server on loopback that answers each request `200 hi` and keeps its head; a
/// connection that sends nothing is no request.
struct Plain {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
}

impl Plain {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let heads: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = Arc::clone(&heads);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                let mut head = Vec::new();
                let mut byte = [0_u8; 1];
                while !head.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
                    head.extend_from_slice(&byte);
                }
                if head.is_empty() {
                    continue;
                }
                kept.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).into_owned());
                drop(conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi"));
            }
        });
        Self { addr, heads }
    }

    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

/// A shared Proxy that lets `plain` through its guard and sends `machine` to it when set.
fn server(w: &World, plain: &Plain, machine: Option<String>) -> Server {
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.guard_exempt.push(plain.addr);
    cfg.machine =
        machine.map(|upstream| MachineRoute::parse(&format!("{MACHINE}={upstream}")).unwrap());
    Server::start(cfg).unwrap()
}

/// Sends `request` into `tunnel` and reads what comes back until the tunnel ends.
fn exchange(mut tunnel: WebSocket<TcpStream>, request: &str) -> String {
    tunnel
        .send(Message::binary(request.as_bytes().to_vec()))
        .unwrap();
    let mut got = Vec::new();
    while let Ok(msg) = tunnel.read() {
        match msg {
            Message::Binary(b) => got.extend_from_slice(&b),
            Message::Close(_) => break,
            _ => {}
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

#[test]
fn a_plain_request_reaches_its_host_in_origin_form_without_proxy_headers() {
    let w = World::new();
    let plain = Plain::start();
    let server = server(&w, &plain, None);
    let target = format!("localhost:{}", plain.addr.port());
    let tunnel = open_as(&server, &w.token, &target, true).unwrap();
    let reply = exchange(
        tunnel,
        &format!(
            "GET http://{target}/x?a=1 HTTP/1.1\r\nHost: {target}\r\n\
             Proxy-Authorization: Basic bW9vcjp4\r\nConnection: keep-alive\r\n\r\n"
        ),
    );
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert!(reply.ends_with("hi"), "{reply}");
    let heads = plain.heads();
    let [head] = heads.as_slice() else {
        unreachable!("{heads:?}")
    };
    assert!(head.starts_with("GET /x?a=1 HTTP/1.1\r\n"), "{head}");
    assert!(
        !head.to_ascii_lowercase().contains("proxy-authorization"),
        "{head}"
    );
    assert!(head.contains("Connection: close\r\n"), "{head}");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_plain_request_to_a_credential_host_carries_no_credential() {
    let w = World::new();
    let plain = Plain::start();
    let server = server(&w, &plain, None);
    let target = w.target();
    let tunnel = open_as(&server, &w.token, &target, true).unwrap();
    let reply = exchange(
        tunnel,
        &format!("GET http://{target}/ok HTTP/1.1\r\nHost: {target}\r\n\r\n"),
    );
    assert!(!reply.starts_with("HTTP/1.1 2"), "{reply}");
    assert!(!reply.contains(super::SECRET), "{reply}");
    assert!(w.up.seen().iter().all(|s| s.authorization.is_none()));
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_plain_request_for_another_host_than_its_tunnel_is_refused() {
    let w = World::new();
    let plain = Plain::start();
    let server = server(&w, &plain, None);
    let target = format!("localhost:{}", plain.addr.port());
    let tunnel = open_as(&server, &w.token, &target, true).unwrap();
    let reply = exchange(
        tunnel,
        "GET http://elsewhere:80/ HTTP/1.1\r\nHost: elsewhere\r\n\r\n",
    );
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    assert!(plain.heads().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn the_machine_service_gets_the_box_identity_and_never_one_the_box_forged() {
    let w = World::new();
    let service = Plain::start();
    let server = server(&w, &service, Some(service.addr.to_string()));
    let tunnel = open_as(&server, &w.token, &format!("{MACHINE}:80"), true).unwrap();
    let reply = exchange(
        tunnel,
        &format!(
            "POST http://{MACHINE}/mcp HTTP/1.1\r\nHost: {MACHINE}\r\nX-Moor-Box: 999\r\n\
             x-moor-machine: forged\r\nContent-Length: 0\r\n\r\n"
        ),
    );
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let heads = service.heads();
    let [head] = heads.as_slice() else {
        unreachable!("{heads:?}")
    };
    assert!(head.starts_with("POST /mcp HTTP/1.1\r\n"), "{head}");
    assert!(
        head.contains(&format!("X-Moor-Box: {}\r\n", w.id.get())),
        "{head}"
    );
    assert!(!head.contains("999") && !head.contains("forged"), "{head}");
    assert!(
        head.contains("X-Moor-Machine-Missing: this Box has no machine yet"),
        "{head}"
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_machine_service_named_by_dns_is_looked_up_when_a_box_calls_it() {
    let w = World::new();
    let service = Plain::start();
    let server = server(
        &w,
        &service,
        Some(format!("localhost:{}", service.addr.port())),
    );
    let tunnel = open_as(&server, &w.token, &format!("{MACHINE}:80"), true).unwrap();
    let reply = exchange(
        tunnel,
        &format!("GET http://{MACHINE}/mcp HTTP/1.1\r\nHost: {MACHINE}\r\n\r\n"),
    );
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert_eq!(service.heads().len(), 1);
    server.drain(Duration::from_secs(5));
}

#[test]
fn the_machine_service_gets_the_service_key_and_never_one_the_box_sent() {
    let w = World::new();
    let service = Plain::start();
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.guard_exempt.push(service.addr);
    cfg.machine = Some(MachineRoute {
        key: Some("the-key".into()),
        ..MachineRoute::parse(&format!("{MACHINE}={}", service.addr)).unwrap()
    });
    let server = Server::start(cfg).unwrap();
    let tunnel = open_as(&server, &w.token, &format!("{MACHINE}:80"), true).unwrap();
    let reply = exchange(
        tunnel,
        &format!(
            "GET http://{MACHINE}/mcp HTTP/1.1\r\nHost: {MACHINE}\r\nX-Moor-Service-Key: forged\r\n\r\n"
        ),
    );
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let heads = service.heads();
    let [head] = heads.as_slice() else {
        unreachable!("{heads:?}")
    };
    assert!(
        head.ends_with("X-Moor-Service-Key: the-key\r\n\r\n"),
        "{head}"
    );
    assert!(!head.contains("forged"), "{head}");
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_machine_service_that_never_accepts_is_502_within_seconds() {
    use rustix::net::{bind, listen, socket, AddressFamily, SocketType};
    let w = World::new();
    let plain = Plain::start();
    let full = socket(AddressFamily::INET, SocketType::STREAM, None).unwrap();
    bind(&full, &SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    listen(&full, 0).unwrap();
    let full = TcpListener::from(full);
    let addr = full.local_addr().unwrap();
    let _queued = TcpStream::connect(addr).unwrap();
    let server = server(&w, &plain, Some(addr.to_string()));
    assert_eq!(
        open_as(&server, &w.token, &format!("{MACHINE}:80"), true).err(),
        Some(502)
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_machine_service_flag_takes_an_ip_or_a_dns_name_with_a_port() {
    for ok in [
        "m=127.0.0.1:7700",
        "m=moor-machine.moor.svc:7700",
        "m=[::1]:7700",
    ] {
        assert!(MachineRoute::parse(ok).is_some(), "{ok}");
    }
    for bad in [
        "m=moor-machine.moor.svc",
        "=127.0.0.1:7700",
        "m=http://x:1",
        "m",
    ] {
        assert!(MachineRoute::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn a_connect_to_the_machine_service_is_refused() {
    let w = World::new();
    let service = Plain::start();
    let server = server(&w, &service, Some(service.addr.to_string()));
    assert_eq!(
        open(&server, &w.token, &format!("{MACHINE}:443")).err(),
        Some(403)
    );
    assert!(service.heads().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_machine_service_that_is_down_is_502() {
    let w = World::new();
    let plain = Plain::start();
    let down = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap();
    let server = server(&w, &plain, Some(down.to_string()));
    assert_eq!(
        open_as(&server, &w.token, &format!("{MACHINE}:80"), true).err(),
        Some(502)
    );
    server.drain(Duration::from_secs(5));
}

/// The proxy credentials a Box can send with a request.
const FORGED: [&str; 4] = [
    "",
    "Proxy-Authorization: Basic bW9vcjp4\r\n",
    "Authorization: Bearer x\r\n",
    "Proxy-Authorization: Basic bW9vcjp4\r\nAuthorization: Bearer x\r\n",
];

#[test]
fn an_origin_form_route_request_is_refused_before_nono_whatever_credential_it_forges() {
    let w = World::new();
    let plain = Plain::start();
    let mut cfg = w.config(64.0, 10.0, 64);
    cfg.guard_exempt.push(plain.addr);
    let server = Server::start(cfg).unwrap();
    let target = w.target();
    let routes = [
        "POST /claude__0/v1/messages",
        "GET /github-git__0/info/refs?service=git-upload-pack",
        "POST /github-api__0/graphql",
    ];
    for route in routes {
        for forged in FORGED {
            let tunnel = open_as(&server, &w.token, &target, true).unwrap();
            let reply = exchange(
                tunnel,
                &format!("{route} HTTP/1.1\r\nHost: {target}\r\n{forged}Content-Length: 0\r\n\r\n"),
            );
            assert!(
                reply.starts_with("HTTP/1.1 403 "),
                "{route} {forged}: {reply}"
            );
            assert!(!reply.contains(super::SECRET), "{reply}");
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(w.up.seen().is_empty(), "the credential host was reached");
    assert!(plain.heads().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_plain_request_to_the_nono_port_is_refused_by_the_guard() {
    let w = World::new();
    let plain = Plain::start();
    let server = server(&w, &plain, None);
    let warm = open_as(&server, &w.token, &w.target(), true).unwrap();
    drop(exchange(
        warm,
        &format!("GET http://{0}/ HTTP/1.1\r\nHost: {0}\r\n\r\n", w.target()),
    ));
    let engines = server.engines();
    let [nono] = engines.as_slice() else {
        unreachable!("{engines:?}")
    };
    let target = format!("127.0.0.1:{}", nono.port());
    assert_eq!(
        open_as(&server, &w.token, &target, true).err(),
        Some(502),
        "the guard refuses the CONNECT into the nono port"
    );
    assert!(w.up.seen().is_empty(), "the credential host was reached");
    server.drain(Duration::from_secs(5));
}
