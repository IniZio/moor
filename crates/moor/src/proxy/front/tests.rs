//! Tests for the CONNECT-only front listener.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use moor_core::Secret;

use super::Front;

const NONCE: &str = "box-nonce-123";
const INTERNAL: &str = "internal-token-456";
const FORWARDED_HEAD: &str = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n";
const WAIT: Duration = Duration::from_secs(5);

/// Standard base64 of `data`, with padding.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = usize::try_from(n >> (18 - 6 * i) & 63).unwrap();
                out.push(char::from(ALPHABET.get(idx).copied().unwrap()));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `moor:<nonce>` Basic credential, base64-encoded.
fn basic() -> String {
    base64(format!("moor:{NONCE}").as_bytes())
}

fn connect_head() -> String {
    let basic = basic();
    format!("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic {basic}\r\n\r\n")
}

/// Fake nono: accepts connections, sends each received byte batch on a channel.
struct Fake {
    addr: SocketAddr,
    seen: Receiver<Vec<u8>>,
}

/// Behaviour of the fake after reading the head.
#[derive(Clone, Copy)]
enum Answer {
    /// Reply 200 then echo every byte.
    Echo,
    /// Reply 407 and close.
    Deny,
}

fn fake(answer: Answer) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, seen) = mpsc::channel();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { return };
            let tx = tx.clone();
            thread::spawn(move || serve_fake(&mut conn, answer, &tx));
        }
    });
    Fake { addr, seen }
}

fn serve_fake(conn: &mut TcpStream, answer: Answer, tx: &mpsc::Sender<Vec<u8>>) {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if conn.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        head.extend_from_slice(&byte);
    }
    tx.send(head).unwrap();
    match answer {
        Answer::Deny => {
            let _ = conn.write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
            );
            let mut buf = [0_u8; 256];
            while let Ok(n) = conn.read(&mut buf) {
                if n == 0 {
                    return;
                }
                tx.send(buf.get(..n).unwrap().to_vec()).unwrap();
            }
        }
        Answer::Echo => {
            conn.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            let mut buf = [0_u8; 256];
            while let Ok(n) = conn.read(&mut buf) {
                if n == 0 {
                    return;
                }
                tx.send(buf.get(..n).unwrap().to_vec()).unwrap();
                conn.write_all(buf.get(..n).unwrap()).unwrap();
            }
        }
    }
}

fn start(target: SocketAddr) -> Front {
    Front::start(
        target,
        Secret::new(NONCE.as_bytes().to_vec()),
        Secret::new(INTERNAL.as_bytes().to_vec()),
    )
    .unwrap()
}

fn dial(front: &Front) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", front.port())).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    s
}

fn read_exact_len(s: &mut TcpStream, len: usize) -> Vec<u8> {
    let mut out = vec![0_u8; len];
    s.read_exact(&mut out).unwrap();
    out
}

fn read_all(s: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// Sends `request`, expects the 403 JSON refusal and no traffic to nono.
fn assert_refused(request: &[u8]) {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let _ = c.write_all(request);
    let reply = read_all(&mut c);
    assert!(reply.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{reply}");
    assert!(reply.contains("Content-Type: application/json\r\n"));
    assert!(reply.contains("Connection: close\r\n"));
    assert!(reply.ends_with(r#"{"message":"moor: only CONNECT is allowed"}"#));
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err(), "nono received bytes");
    front.stop();
}

#[test]
fn connect_head_is_forwarded_with_internal_token_and_tunnel_echoes() {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let mut wire = connect_head().into_bytes();
    wire.extend_from_slice(b"early");
    c.write_all(&wire).unwrap();
    assert_eq!(
        nono.seen.recv_timeout(WAIT).unwrap(),
        FORWARDED_HEAD.as_bytes()
    );
    let status = b"HTTP/1.1 200 Connection Established\r\n\r\n";
    assert_eq!(read_exact_len(&mut c, status.len()), status);
    assert_eq!(nono.seen.recv_timeout(WAIT).unwrap(), b"early");
    assert_eq!(read_exact_len(&mut c, 5), b"early");
    c.write_all(b"ping").unwrap();
    assert_eq!(read_exact_len(&mut c, 4), b"ping");
    front.stop();
}

#[test]
fn refused_connect_closes_and_drops_later_client_bytes() {
    let nono = fake(Answer::Deny);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let mut wire = connect_head().into_bytes();
    wire.extend_from_slice(b"GET /smuggled HTTP/1.1\r\n\r\n");
    c.write_all(&wire).unwrap();
    let reply = read_all(&mut c);
    assert!(reply.starts_with("HTTP/1.1 407 "), "{reply}");
    assert_eq!(
        nono.seen.recv_timeout(WAIT).unwrap(),
        FORWARDED_HEAD.as_bytes()
    );
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err(), "extra bytes reached nono");
    front.stop();
}

#[test]
fn origin_form_reverse_request_is_refused() {
    let basic = basic();
    assert_refused(
        format!("POST /github-api__0/graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nProxy-Authorization: Basic {basic}\r\nAuthorization: Bearer x\r\nContent-Length: 0\r\n\r\n").as_bytes(),
    );
}

#[test]
fn absolute_form_requests_are_refused() {
    assert_refused(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n");
    let nono = fake(Answer::Echo);
    let req = format!("GET http://{}/x HTTP/1.1\r\nHost: x\r\n\r\n", nono.addr);
    let front = start(nono.addr);
    let mut c = dial(&front);
    c.write_all(req.as_bytes()).unwrap();
    assert!(read_all(&mut c).starts_with("HTTP/1.1 403 Forbidden"));
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err());
    front.stop();
}

#[test]
fn lowercase_method_is_refused() {
    assert_refused(b"connect example.com:443 HTTP/1.1\r\nHost: x\r\n\r\n");
}

#[test]
fn connect_with_path_target_is_refused() {
    assert_refused(b"CONNECT /x HTTP/1.1\r\nHost: x\r\n\r\n");
}

#[test]
fn connect_with_path_in_authority_is_refused() {
    assert_refused(b"CONNECT example.com:443/x HTTP/1.1\r\nHost: x\r\n\r\n");
}

#[test]
fn oversize_head_is_refused() {
    let mut req = b"CONNECT example.com:443 HTTP/1.1\r\nX-Pad: ".to_vec();
    req.extend(std::iter::repeat_n(b'a', 9000));
    req.extend_from_slice(b"\r\n\r\n");
    assert_refused(&req);
}

#[test]
fn bare_lf_head_is_refused() {
    assert_refused(b"CONNECT example.com:443 HTTP/1.1\nHost: x\n\r\n\r\n");
}

#[test]
fn obs_fold_head_is_refused() {
    assert_refused(b"CONNECT example.com:443 HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n");
}

#[test]
fn stop_returns_promptly_and_port_refuses() {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let port = front.port();
    assert!(format!("{front:?}").contains(&port.to_string()));
    front.stop();
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

/// Sends `request`, expects the 407 reply and no traffic to nono.
fn assert_unauthorized(request: &str) {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let _ = c.write_all(request.as_bytes());
    let reply = read_all(&mut c);
    assert!(
        reply.starts_with("HTTP/1.1 407 Proxy Authentication Required\r\n"),
        "{reply}"
    );
    assert!(reply.contains("Proxy-Authenticate: Basic realm=\"moor\"\r\n"));
    assert!(reply.contains("Content-Type: application/json\r\n"));
    assert!(reply.contains("Connection: close\r\n"));
    assert!(reply.ends_with(r#"{"message":"moor: proxy authentication required"}"#));
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err(), "nono received bytes");
    front.stop();
}

fn connect_with(headers: &str) -> String {
    format!("CONNECT example.com:443 HTTP/1.1\r\n{headers}\r\n")
}

#[test]
fn basic_credential_is_replaced_and_other_headers_keep_order() {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let basic = basic();
    let req = connect_with(&format!(
        "Host: example.com:443\r\nproxy-authorization: Basic {basic}\r\nUser-Agent: t\r\nX-Last:  v \r\n"
    ));
    c.write_all(req.as_bytes()).unwrap();
    let seen = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        seen,
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nUser-Agent: t\r\nX-Last:  v \r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n"
    );
    assert!(!seen.contains(NONCE) && !seen.contains(&basic));
    front.stop();
}

#[test]
fn bearer_credential_is_replaced() {
    let nono = fake(Answer::Echo);
    let front = start(nono.addr);
    let mut c = dial(&front);
    let req = connect_with(&format!(
        "Proxy-Authorization: Bearer {NONCE}\r\nHost: h\r\n"
    ));
    c.write_all(req.as_bytes()).unwrap();
    let seen = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        seen,
        "CONNECT example.com:443 HTTP/1.1\r\nHost: h\r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n"
    );
    front.stop();
}

#[test]
fn wrong_basic_password_is_rejected() {
    // moor:box-nonce-124
    assert_unauthorized(&connect_with(
        "Proxy-Authorization: Basic bW9vcjpib3gtbm9uY2UtMTI0\r\n",
    ));
}

#[test]
fn wrong_bearer_is_rejected() {
    assert_unauthorized(&connect_with("Proxy-Authorization: Bearer nope\r\n"));
    assert_unauthorized(&connect_with(&format!(
        "Proxy-Authorization: Bearer {NONCE}x\r\n"
    )));
}

#[test]
fn internal_token_from_the_box_is_rejected() {
    assert_unauthorized(&connect_with(&format!(
        "Proxy-Authorization: Bearer {INTERNAL}\r\n"
    )));
}

#[test]
fn missing_credential_is_rejected() {
    assert_unauthorized(&connect_with("Host: example.com:443\r\n"));
}

#[test]
fn two_credentials_are_rejected() {
    let basic = basic();
    assert_unauthorized(&connect_with(&format!(
        "Proxy-Authorization: Basic {basic}\r\nProxy-Authorization: Bearer {NONCE}\r\n"
    )));
}

#[test]
fn malformed_base64_is_rejected() {
    assert_unauthorized(&connect_with(
        "Proxy-Authorization: Basic bW9vcjpib3gtbm9uY2UtMTI*\r\n",
    ));
    assert_unauthorized(&connect_with(
        "Proxy-Authorization: Basic bW9vcjpib3gtbm9uY2UtMTI\r\n",
    ));
    assert_unauthorized(&connect_with(&format!(
        "Proxy-Authorization: Basic ={}\r\n",
        basic().chars().skip(1).collect::<String>()
    )));
    assert_unauthorized(&connect_with("Proxy-Authorization: Basic\r\n"));
}

#[test]
fn non_connect_with_valid_credential_is_refused() {
    let basic = basic();
    assert_refused(
        format!("GET api.github.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {basic}\r\n\r\n")
            .as_bytes(),
    );
}
