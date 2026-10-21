//! Tests for the front listener.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use moor_core::{BoxId, Secret};
use tempfile::TempDir;

use super::{Front, FrontSocket};

const INTERNAL: &str = "internal-token-456";
const FORWARDED_HEAD: &str = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n";
const WAIT: Duration = Duration::from_secs(5);

/// A CONNECT as the Box sends it: no proxy credential.
fn connect_head() -> String {
    "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n".to_owned()
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
    /// Read the head and never answer.
    Hang,
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
        Answer::Hang => {
            let mut buf = [0_u8; 256];
            while conn.read(&mut buf).unwrap_or(0) > 0 {}
        }
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

/// The uid this test runs as.
fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// The front socket of Box 1 in `dir`'s private socket directory `s`, which `owner` must own.
fn socket_in(dir: &TempDir, owner: u32) -> FrontSocket {
    FrontSocket::new(
        &dir.path().join("s"),
        owner,
        Path::new("/state"),
        BoxId::new(1),
    )
}

/// A front for `target`; the temp dir holds its socket and must outlive it.
fn start(target: SocketAddr) -> (TempDir, Front) {
    let dir = tempfile::tempdir().unwrap();
    let front = Front::start(
        target,
        socket_in(&dir, uid()),
        Secret::new(INTERNAL.as_bytes().to_vec()),
    )
    .unwrap();
    (dir, front)
}

fn dial(front: &Front) -> UnixStream {
    let s = UnixStream::connect(front.path()).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    s
}

fn read_exact_len(s: &mut UnixStream, len: usize) -> Vec<u8> {
    let mut out = vec![0_u8; len];
    s.read_exact(&mut out).unwrap();
    out
}

fn read_all(s: &mut UnixStream) -> String {
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// Sends `request`, expects the 403 JSON refusal and no traffic to nono.
fn assert_refused(request: &[u8]) {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    let _ = c.write_all(request);
    let reply = read_all(&mut c);
    assert!(reply.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{reply}");
    assert!(reply.contains("Content-Type: application/json\r\n"));
    assert!(reply.contains("Connection: close\r\n"));
    assert!(reply.ends_with(r#"{"message":"moor: only CONNECT or http:// requests are allowed"}"#));
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err(), "nono received bytes");
    front.stop();
}

#[test]
fn connect_head_is_forwarded_with_internal_token_and_tunnel_echoes() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
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
    let (_dir, front) = start(nono.addr);
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
    assert_refused(
        b"POST /github-api__0/graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer x\r\nContent-Length: 0\r\n\r\n",
    );
}

#[test]
fn origin_form_and_https_absolute_requests_are_refused() {
    assert_refused(b"GET /x HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_refused(b"GET https://example.com/ HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_refused(b"GET http://u:p@example.com/ HTTP/1.1\r\nHost: x\r\n\r\n");
    assert_refused(b"GET http://example.com/#f HTTP/1.1\r\nHost: x\r\n\r\n");
}

#[test]
fn plain_get_becomes_connect_then_origin_form_request() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    let req = "GET http://example.com/x?a=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic bW9vcjp4\r\nProxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\nX-Hop: 1\r\nAccept: */*\r\n\r\n";
    c.write_all(req.as_bytes()).unwrap();
    let connect = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        connect,
        "CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n"
    );
    let tunneled = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        tunneled,
        "GET /x?a=1 HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(read_exact_len(&mut c, tunneled.len()), tunneled.as_bytes());
    front.stop();
}

#[test]
fn plain_post_body_and_default_host_arrive_intact() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    let req = "POST http://example.com:8080 HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody";
    c.write_all(req.as_bytes()).unwrap();
    let connect = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert!(
        connect.starts_with("CONNECT example.com:8080 HTTP/1.1\r\n"),
        "{connect}"
    );
    let head = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        head,
        "POST / HTTP/1.1\r\nContent-Length: 4\r\nHost: example.com:8080\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(nono.seen.recv_timeout(WAIT).unwrap(), b"body");
    front.stop();
}

#[test]
fn connection_header_cannot_drop_host_or_content_length() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    let req = "POST http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: host, content-length\r\nContent-Length: 4\r\n\r\nbody";
    c.write_all(req.as_bytes()).unwrap();
    let _connect = nono.seen.recv_timeout(WAIT).unwrap();
    let head = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        head,
        "POST / HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4\r\nConnection: close\r\n\r\n"
    );
    front.stop();
}

#[test]
fn refused_plain_request_is_relayed_and_nothing_enters_the_tunnel() {
    let nono = fake(Answer::Deny);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    let req = "POST http://example.com/ HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody";
    c.write_all(req.as_bytes()).unwrap();
    let reply = read_all(&mut c);
    assert!(reply.starts_with("HTTP/1.1 407 "), "{reply}");
    assert!(nono
        .seen
        .recv_timeout(WAIT)
        .unwrap()
        .starts_with(b"CONNECT example.com:80 "));
    thread::sleep(Duration::from_millis(200));
    assert!(nono.seen.try_recv().is_err(), "request entered the tunnel");
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
fn stop_returns_promptly_and_removes_the_socket() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let path = front.path().to_path_buf();
    assert!(format!("{front:?}").contains(path.to_str().unwrap()));
    front.stop();
    assert!(!path.exists());
    assert!(UnixStream::connect(&path).is_err());
}

/// Each stopping front's acceptor is still in its poll when the successor binds.
#[test]
fn a_successor_bound_while_a_front_stops_keeps_its_socket() {
    let nono = fake(Answer::Echo);
    let (dir, mut front) = start(nono.addr);
    for _ in 0..3 {
        let stopping = thread::spawn(move || front.stop());
        thread::sleep(Duration::from_millis(20));

        front = Front::start(
            nono.addr,
            socket_in(&dir, uid()),
            Secret::new(INTERNAL.as_bytes().to_vec()),
        )
        .unwrap();
        stopping.join().unwrap();

        let mut c = dial(&front);
        c.write_all(connect_head().as_bytes()).unwrap();
        assert_eq!(
            nono.seen.recv_timeout(WAIT).unwrap(),
            FORWARDED_HEAD.as_bytes()
        );
    }
    front.stop();
}

#[test]
fn each_box_and_each_state_dir_gets_its_own_socket() {
    let at = |state: &str, id: i64| {
        let socket = FrontSocket::new(Path::new("/s"), 0, Path::new(state), BoxId::new(id));
        socket.path().to_path_buf()
    };
    assert_eq!(at("/a", 1), at("/a", 1));
    assert_ne!(at("/a", 1), at("/a", 2));
    assert_ne!(at("/a", 1), at("/b", 1));
}

/// A hard link reaches a listener whose path is gone, so it shows the listener closed.
#[test]
fn rebind_and_stop_close_the_listener_they_replace() {
    let nono = fake(Answer::Echo);
    let (dir, front) = start(nono.addr);
    let path = front.path().to_path_buf();
    let first = dir.path().join("first");
    std::fs::hard_link(&path, &first).unwrap();
    std::fs::remove_file(&path).unwrap();

    front.rebind().unwrap();

    assert!(
        UnixStream::connect(&first).is_err(),
        "replaced listener accepts"
    );
    let second = dir.path().join("second");
    std::fs::hard_link(&path, &second).unwrap();
    assert!(UnixStream::connect(&second).is_ok());
    front.stop();
    assert!(
        UnixStream::connect(&second).is_err(),
        "stopped listener accepts"
    );
}

fn connect_with(headers: &str) -> String {
    format!("CONNECT example.com:443 HTTP/1.1\r\n{headers}\r\n")
}

#[test]
fn every_proxy_credential_from_the_box_is_replaced_by_the_internal_token() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    for sent in [
        "",
        "Proxy-Authorization: Basic bW9vcjp4\r\n",
        "proxy-authorization: Bearer made-up\r\n",
        "Proxy-Authorization: Bearer internal-token-456\r\n",
        "Proxy-Authorization: Basic *\r\nProxy-Authorization: Bearer two\r\n",
    ] {
        let mut c = dial(&front);
        let req = connect_with(&format!(
            "Host: h\r\n{sent}User-Agent: t\r\nX-Last:  v \r\n"
        ));
        c.write_all(req.as_bytes()).unwrap();
        let seen = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
        assert_eq!(
            seen,
            "CONNECT example.com:443 HTTP/1.1\r\nHost: h\r\nUser-Agent: t\r\nX-Last:  v \r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n",
            "sent {sent:?}"
        );
    }
    front.stop();
}

#[test]
fn plain_request_without_a_credential_reaches_nono_with_the_internal_token() {
    let nono = fake(Answer::Echo);
    let (_dir, front) = start(nono.addr);
    let mut c = dial(&front);
    c.write_all(b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n")
        .unwrap();
    let connect = String::from_utf8(nono.seen.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        connect,
        "CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\nProxy-Authorization: Bearer internal-token-456\r\n\r\n"
    );
    front.stop();
}

#[test]
fn non_connect_authority_target_is_refused() {
    assert_refused(b"GET api.github.com:443 HTTP/1.1\r\nHost: x\r\n\r\n");
}

#[test]
fn a_file_left_at_the_socket_path_is_replaced() {
    let nono = fake(Answer::Echo);
    let dir = tempfile::tempdir().unwrap();
    let socket = socket_in(&dir, uid());
    std::fs::create_dir(dir.path().join("s")).unwrap();
    std::fs::set_permissions(dir.path().join("s"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(socket.path(), b"stale").unwrap();

    let front = Front::start(
        nono.addr,
        socket.clone(),
        Secret::new(INTERNAL.as_bytes().to_vec()),
    )
    .unwrap();

    assert!(std::fs::symlink_metadata(socket.path())
        .unwrap()
        .file_type()
        .is_socket());
    let mut c = dial(&front);
    c.write_all(connect_head().as_bytes()).unwrap();
    assert_eq!(
        nono.seen.recv_timeout(WAIT).unwrap(),
        FORWARDED_HEAD.as_bytes()
    );
    front.stop();
}

#[test]
fn a_nono_that_never_answers_gets_the_client_a_bounded_refusal() {
    let nono = fake(Answer::Hang);
    let (_dir, front) = start(nono.addr);
    let mut client = dial(&front);
    client.write_all(connect_head().as_bytes()).unwrap();
    let reply = read_all(&mut client);
    assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    assert!(reply.ends_with(super::REFUSAL_BODY), "{reply}");
    assert_eq!(
        nono.seen.recv_timeout(WAIT).unwrap(),
        FORWARDED_HEAD.as_bytes()
    );
}
