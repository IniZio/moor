//! Tests for request parsing, response headers and the server over a stub handler.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions slice parsed buffers and unwrap I/O"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

/// Yields one scripted slice per `read` call.
struct Pieces(Vec<Vec<u8>>);

impl Read for Pieces {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.0.is_empty() {
            return Ok(0);
        }
        let piece = self.0.remove(0);
        out[..piece.len()].copy_from_slice(&piece);
        Ok(piece.len())
    }
}

fn read(pieces: &[&[u8]]) -> io::Result<HttpRequest> {
    read_request(&mut Pieces(pieces.iter().map(|p| p.to_vec()).collect()))
}

#[test]
fn body_of_exact_length_arrives_with_head() {
    let req = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]).unwrap();
    assert_eq!(req.body, b"hello");
}

#[test]
fn body_split_across_reads_is_joined() {
    let req = read(&[
        b"POST /x HTTP/1.1\r\nContent-Length: 6\r\n\r\nab",
        b"cd",
        b"ef",
    ])
    .unwrap();
    assert_eq!(req.body, b"abcdef");
}

#[test]
fn head_split_mid_terminator_keeps_leftover() {
    let req = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 2\r\n\r", b"\nok"]).unwrap();
    assert_eq!(req.body, b"ok");
}

#[test]
fn missing_content_length_gives_empty_body() {
    let req = read(&[b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n"]).unwrap();
    assert!(req.body.is_empty());
}

#[test]
fn oversize_body_is_invalid_input() {
    let head = format!(
        "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    let err = read(&[head.as_bytes()]).err().unwrap();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn chunked_is_invalid_data() {
    let err = read(&[b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"])
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn unparsable_length_is_invalid_data() {
    let err = read(&[b"POST /x HTTP/1.1\r\nContent-Length: abc\r\n\r\n"])
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn short_body_is_unexpected_eof() {
    let err = read(&[b"POST /x HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc"])
        .err()
        .unwrap();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn csp_forbids_forms_framing_and_base() {
    for d in [
        "form-action 'none'",
        "frame-ancestors 'none'",
        "base-uri 'none'",
    ] {
        assert!(COMMON_HEADERS.contains(d), "{d}");
    }
}

/// Token the test server requires.
const TOKEN: &str = "s3cret-token";

/// Paths of the requests the stub handler ran.
type Seen = Arc<Mutex<Vec<String>>>;

/// Starts a server whose handler serves a page, echoes POST bodies at `/echo` and 404s the rest.
fn server() -> (Server, Seen) {
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    let handler: Handler = Arc::new(move |req| {
        log.lock().unwrap().push(req.path.clone());
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => HttpResponse::html("<h1>page</h1>").into(),
            ("GET", "/stream") => {
                let log = Arc::clone(&log);
                Reply::Stream(Box::new(move |out| {
                    log.lock().unwrap().push("stream body".into());
                    out.write_all(b"data: one\n\n")?;
                    out.write_all(b"data: two\n\n")
                }))
            }
            ("POST", "/echo") => {
                HttpResponse::text("200 OK", String::from_utf8_lossy(&req.body).into_owned()).into()
            }
            _ => HttpResponse::text("404 Not Found", "not found").into(),
        }
    });
    (start(0, TOKEN.into(), handler).unwrap(), seen)
}

/// Writes a request head with `method` and returns the open stream; `Host` defaults to the bound address.
fn send_head(web: &Server, method: &str, path: &str, headers: &[&str]) -> TcpStream {
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\n").unwrap();
    if !headers.iter().any(|h| h.starts_with("Host:")) {
        write!(s, "Host: {}\r\n", web.addr()).unwrap();
    }
    for h in headers {
        write!(s, "{h}\r\n").unwrap();
    }
    write!(s, "\r\n").unwrap();
    s
}

/// Fetches a whole response (the server closes after it).
fn fetch(web: &Server, method: &str, path: &str, headers: &[&str]) -> String {
    let mut out = String::new();
    send_head(web, method, path, headers)
        .read_to_string(&mut out)
        .unwrap();
    out
}

/// POSTs `body` and returns the whole response; adds `Content-Length` unless given.
fn post(web: &Server, path: &str, headers: &[&str], body: &str) -> String {
    let len = format!("Content-Length: {}", body.len());
    let mut all = headers.to_vec();
    if !headers.iter().any(|h| h.starts_with("Content-Length")) {
        all.push(&len);
    }
    let mut s = send_head(web, "POST", path, &all);
    // The server may answer and close before reading an oversize body.
    let _ = s.write_all(body.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

fn bearer() -> String {
    format!("Authorization: Bearer {TOKEN}")
}

const JSON: &str = "Content-Type: application/json";
const CSRF: &str = "X-Moor-Csrf: 1";

#[test]
fn missing_or_wrong_token_is_unauthorized_everywhere() {
    let (web, seen) = server();
    for path in ["/", "/echo", "/nope"] {
        for method in ["GET", "POST", "PUT"] {
            let anon = fetch(&web, method, path, &[]);
            assert!(anon.starts_with("HTTP/1.1 401"), "{method} {path}");
            let wrong = fetch(
                &web,
                method,
                path,
                &["Authorization: Bearer nope", "Cookie: moor_token=nope"],
            );
            assert!(wrong.starts_with("HTTP/1.1 401"), "{method} {path}");
            assert!(wrong.ends_with("unauthorized"));
        }
    }
    let query_off_page = fetch(&web, "GET", &format!("/nope?token={TOKEN}"), &[]);
    assert!(query_off_page.starts_with("HTTP/1.1 401"));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn binds_loopback_only() {
    let (web, _) = server();
    assert!(web.addr().ip().is_loopback());
}

#[test]
fn port_zero_picks_a_port_and_url_names_it() {
    let (web, _) = server();
    assert_ne!(web.addr().port(), 0);
    assert_eq!(
        web.url(),
        format!("http://127.0.0.1:{}/", web.addr().port())
    );
}

#[test]
fn page_is_served_with_credentials_and_security_headers() {
    let (web, _) = server();
    let page = fetch(&web, "GET", "/", &[&bearer()]);
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("text/html"));
    assert!(page.contains("X-Content-Type-Options: nosniff"));
    assert!(page.contains("Content-Security-Policy: default-src 'none'"));
    assert!(page.ends_with("<h1>page</h1>"));
    assert!(!page.contains("Set-Cookie"));
}

#[test]
fn placeholder_handler_serves_root_and_404s_the_rest() {
    let web = start(0, TOKEN.into(), placeholder_handler()).unwrap();
    let page = fetch(&web, "GET", "/", &[&bearer()]);
    assert!(page.starts_with("HTTP/1.1 200") && page.contains("<title>moor</title>"));
    assert!(fetch(&web, "GET", "/x", &[&bearer()]).starts_with("HTTP/1.1 404"));
}

#[test]
fn query_token_on_page_sets_cookie() {
    let (web, _) = server();
    let page = fetch(&web, "GET", &format!("/?token={TOKEN}"), &[]);
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains(&format!("Set-Cookie: {}", auth::cookie_header(TOKEN))));
    let cookie = format!("Cookie: moor_token={TOKEN}");
    assert!(fetch(&web, "GET", "/", &[&cookie]).starts_with("HTTP/1.1 200"));
}

#[test]
fn other_methods_and_paths_are_rejected_after_auth() {
    let (web, _) = server();
    let cookie = format!("Cookie: moor_token={TOKEN}");
    let put = fetch(&web, "PUT", "/", &[&cookie]);
    assert!(put.starts_with("HTTP/1.1 405"));
    assert!(put.contains("Allow: GET, POST"));
    assert!(fetch(&web, "GET", "/nope", &[&cookie]).starts_with("HTTP/1.1 404"));
}

#[test]
fn post_reaches_the_handler_with_its_body() {
    let (web, seen) = server();
    let resp = post(&web, "/echo", &[&bearer(), JSON, CSRF], r#"{"a":1}"#);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.ends_with(r#"{"a":1}"#));
    assert_eq!(*seen.lock().unwrap(), ["/echo"]);
}

#[test]
fn post_needs_a_header_credential() {
    let (web, seen) = server();
    let anon = post(&web, "/echo", &[JSON, CSRF], "{}");
    assert!(anon.starts_with("HTTP/1.1 401"), "{anon}");
    let query = post(&web, &format!("/echo?token={TOKEN}"), &[JSON, CSRF], "{}");
    assert!(query.starts_with("HTTP/1.1 401"), "{query}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn csrf_requirements_are_enforced() {
    let (web, seen) = server();
    let auth = bearer();
    let rejected: [&[&str]; 6] = [
        &[&auth, JSON],
        &[&auth, JSON, CSRF, "Origin: http://evil.example"],
        &[&auth, JSON, CSRF, "Sec-Fetch-Site: cross-site"],
        &[&auth, "Content-Type: text/plain", CSRF],
        &[&auth, JSON, CSRF, "Host: evil.example:80"],
        &[&auth, JSON, CSRF, "Host: 127.0.0.1.evil.example"],
    ];
    for headers in rejected {
        let resp = post(&web, "/echo", headers, "{}");
        assert!(resp.starts_with("HTTP/1.1 403"), "{headers:?}: {resp}");
        assert!(resp.contains("forbidden"));
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn oversize_body_is_rejected() {
    let (web, seen) = server();
    let len = format!("Content-Length: {}", MAX_BODY + 1);
    let resp = post(&web, "/echo", &[&bearer(), JSON, CSRF, &len], "");
    assert!(resp.starts_with("HTTP/1.1 413"), "{resp}");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn malformed_request_line_is_a_bad_request() {
    let (web, _) = server();
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.write_all(b"junk\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
}

#[test]
fn stream_reply_sends_event_stream_head_then_frames() {
    let (web, _) = server();
    let resp = fetch(&web, "GET", "/stream", &[&bearer()]);
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
    assert!(resp.contains("Content-Type: text/event-stream"));
    assert!(resp.contains("Cache-Control: no-store"));
    assert!(resp.contains("Connection: close"));
    assert!(resp.contains("Content-Security-Policy: default-src 'none'"));
    assert!(!resp.contains("Content-Length"));
    assert!(
        resp.ends_with("\r\n\r\ndata: one\n\ndata: two\n\n"),
        "{resp}"
    );
}

#[test]
fn unauthenticated_stream_is_refused_and_never_runs() {
    let (web, seen) = server();
    let resp = fetch(&web, "GET", "/stream", &[]);
    assert!(resp.starts_with("HTTP/1.1 401"), "{resp}");
    assert!(!resp.contains("event-stream"));
    assert!(seen.lock().unwrap().is_empty());
}
