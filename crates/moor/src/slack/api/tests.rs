//! Tests for the Slack Web API client against a loopback HTTP responder.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::secret::Secret;

use super::{Api, ApiError};

/// A token whose text must never show up in an error.
const TOKEN: &str = "xoxb-super-secret-token";

/// Serves one HTTP reply on loopback; the handle yields the raw request received.
fn serve(reply: &'static str) -> (String, JoinHandle<String>) {
    serve_status("200 OK", reply)
}

/// Like [`serve`] but with an explicit HTTP status line.
fn serve_status(status: &'static str, reply: &'static str) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept");
        conn.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut buf = [0_u8; 4096];
        while !request_complete(&request) {
            let n = conn.read(&mut buf).expect("read");
            assert!(n > 0, "client closed early");
            request.extend_from_slice(buf.get(..n).expect("slice"));
        }
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        conn.write_all(response.as_bytes()).expect("write");
        String::from_utf8(request).expect("utf8")
    });
    (base, handle)
}

/// True once the headers and the declared body have arrived.
fn request_complete(request: &[u8]) -> bool {
    let text = String::from_utf8_lossy(request);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let wanted = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    body.len() >= wanted
}

#[test]
fn open_connection_returns_the_url_and_sends_the_app_token() {
    let (base, handle) = serve(r#"{"ok":true,"url":"wss://wss-primary.slack.com/link/?ticket=t"}"#);
    let api = Api::new(&base).expect("api");
    let url = api
        .open_connection(&Secret::new(TOKEN.as_bytes().to_vec()))
        .expect("open");
    assert_eq!(url, "wss://wss-primary.slack.com/link/?ticket=t");
    let request = handle.join().expect("server");
    assert!(
        request.starts_with("POST /apps.connections.open "),
        "{request}"
    );
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
}

#[test]
fn ok_false_maps_to_a_slack_error() {
    let (base, handle) = serve(r#"{"ok":false,"error":"invalid_auth"}"#);
    let api = Api::new(&base).expect("api");
    let err = api
        .open_connection(&Secret::new(TOKEN.as_bytes().to_vec()))
        .expect_err("rejected");
    handle.join().expect("server");
    assert!(
        matches!(&err, ApiError::Slack(code) if code == "invalid_auth"),
        "{err:?}"
    );
}

#[test]
fn post_message_sends_the_bot_token_and_json_body() {
    let (base, handle) = serve(r#"{"ok":true,"ts":"1730000000.2"}"#);
    let api = Api::new(&base).expect("api");
    let ts = api
        .post_message(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1730000000.1",
            "hi \"there\"",
        )
        .expect("post");
    assert_eq!(ts, "1730000000.2");
    let request = handle.join().expect("server");
    assert!(request.starts_with("POST /chat.postMessage "), "{request}");
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json; charset=utf-8"),
        "{request}"
    );
    let body = request.split_once("\r\n\r\n").expect("body").1;
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    assert_eq!(
        value,
        serde_json::json!({"channel":"C1","thread_ts":"1730000000.1","text":"hi \"there\""})
    );
}

#[test]
fn post_message_without_ts_is_a_slack_error() {
    let (base, handle) = serve(r#"{"ok":true}"#);
    let api = Api::new(&base).expect("api");
    let err = api
        .post_message(&Secret::new(TOKEN.as_bytes().to_vec()), "C1", "1.2", "x")
        .expect_err("no ts");
    handle.join().expect("server");
    assert!(matches!(err, ApiError::Slack(_)), "{err:?}");
}

#[test]
fn update_message_sends_the_bot_token_and_json_body() {
    let (base, handle) = serve(r#"{"ok":true,"ts":"1730000000.2"}"#);
    let api = Api::new(&base).expect("api");
    api.update_message(
        &Secret::new(TOKEN.as_bytes().to_vec()),
        "C1",
        "1730000000.2",
        "done",
    )
    .expect("update");
    let request = handle.join().expect("server");
    assert!(request.starts_with("POST /chat.update "), "{request}");
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
    let body = request.split_once("\r\n\r\n").expect("body").1;
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    assert_eq!(
        value,
        serde_json::json!({"channel":"C1","ts":"1730000000.2","text":"done"})
    );
}

/// The JSON body of a raw HTTP request.
fn json_body(request: &str) -> serde_json::Value {
    let body = request.split_once("\r\n\r\n").expect("body").1;
    serde_json::from_str(body).expect("json")
}

#[test]
fn post_blocks_sends_blocks_and_returns_ts() {
    let (base, handle) = serve(r#"{"ok":true,"ts":"1730000000.3"}"#);
    let api = Api::new(&base).expect("api");
    let blocks = serde_json::json!([{"type":"section","text":{"type":"mrkdwn","text":"q"}}]);
    let ts = api
        .post_blocks(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1.1",
            "q",
            &blocks,
        )
        .expect("post");
    assert_eq!(ts, "1730000000.3");
    let request = handle.join().expect("server");
    assert!(request.starts_with("POST /chat.postMessage "), "{request}");
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
    assert_eq!(
        json_body(&request),
        serde_json::json!({"channel":"C1","thread_ts":"1.1","text":"q","blocks":blocks})
    );
}

#[test]
fn update_blocks_sends_blocks() {
    let (base, handle) = serve(r#"{"ok":true}"#);
    let api = Api::new(&base).expect("api");
    let blocks = serde_json::json!([{"type":"divider"}]);
    api.update_blocks(
        &Secret::new(TOKEN.as_bytes().to_vec()),
        "C1",
        "1.2",
        "done",
        &blocks,
    )
    .expect("update");
    let request = handle.join().expect("server");
    assert!(request.starts_with("POST /chat.update "), "{request}");
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
    assert_eq!(
        json_body(&request),
        serde_json::json!({"channel":"C1","ts":"1.2","text":"done","blocks":blocks})
    );
}

#[test]
fn post_ephemeral_sends_user_and_thread() {
    let (base, handle) = serve(r#"{"ok":true,"message_ts":"1.9"}"#);
    let api = Api::new(&base).expect("api");
    api.post_ephemeral(
        &Secret::new(TOKEN.as_bytes().to_vec()),
        "C1",
        "U1",
        "1.1",
        "not yours",
    )
    .expect("ephemeral");
    let request = handle.join().expect("server");
    assert!(
        request.starts_with("POST /chat.postEphemeral "),
        "{request}"
    );
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
    );
    assert_eq!(
        json_body(&request),
        serde_json::json!({"channel":"C1","user":"U1","thread_ts":"1.1","text":"not yours"})
    );
}

#[test]
fn block_calls_map_ok_false_to_a_slack_error() {
    let token = Secret::new(TOKEN.as_bytes().to_vec());
    let blocks = serde_json::json!([]);
    for call in 0..3 {
        let (base, handle) = serve(r#"{"ok":false,"error":"channel_not_found"}"#);
        let api = Api::new(&base).expect("api");
        let err = match call {
            0 => api.post_blocks(&token, "C1", "1.1", "q", &blocks).map(drop),
            1 => api.update_blocks(&token, "C1", "1.2", "q", &blocks),
            _ => api.post_ephemeral(&token, "C1", "U1", "1.1", "q"),
        }
        .expect_err("rejected");
        handle.join().expect("server");
        assert!(
            matches!(&err, ApiError::Slack(code) if code == "channel_not_found"),
            "{err:?}"
        );
    }
}

#[test]
fn http_429_maps_to_rate_limited() {
    let (base, handle) = serve_status(
        "429 Too Many Requests",
        r#"{"ok":false,"error":"ratelimited"}"#,
    );
    let api = Api::new(&base).expect("api");
    let err = api
        .update_message(&Secret::new(TOKEN.as_bytes().to_vec()), "C1", "1.2", "x")
        .expect_err("limited");
    handle.join().expect("server");
    assert!(matches!(err, ApiError::RateLimited), "{err:?}");
    assert!(err.to_string().contains("rate limited"));
}

#[test]
fn errors_never_contain_the_token() {
    let (base, handle) = serve("not json");
    let api = Api::new(&base).expect("api");
    let token = Secret::new(TOKEN.as_bytes().to_vec());
    let garbled = api.open_connection(&token).expect_err("garbled");
    handle.join().expect("server");
    let dead = Api::new("http://127.0.0.1:1").expect("api");
    let refused = dead
        .post_message(&token, "C1", "1.2", "x")
        .expect_err("refused");
    let refused_update = dead
        .update_message(&token, "C1", "1.2", "x")
        .expect_err("refused");
    for err in [
        garbled,
        refused,
        refused_update,
        ApiError::Slack("invalid_auth".into()),
        ApiError::RateLimited,
    ] {
        assert!(!format!("{err} {err:?}").contains(TOKEN));
    }
}

#[test]
fn rejects_a_base_url_without_http_scheme() {
    assert!(matches!(
        Api::new("ftp://slack.com"),
        Err(ApiError::Transport(_))
    ));
}

/// One request a [`serve_routes`] fake received.
struct Seen {
    /// Request path.
    path: String,
    /// Lower-cased header block, including the request line.
    head: String,
    /// Request body.
    body: Vec<u8>,
}

/// Serves `count` requests on loopback, answering each by path from `routes`.
///
/// A route reply of `("200 OK", text)`; `{BASE}` in `text` becomes the fake's base URL.
fn serve_routes(
    count: usize,
    routes: &'static [(&'static str, &'static str, &'static str)],
) -> (String, JoinHandle<Vec<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let own = base.clone();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..count {
            let (mut conn, _) = listener.accept().expect("accept");
            conn.set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            while !request_complete(&request) {
                let n = conn.read(&mut buf).expect("read");
                assert!(n > 0, "client closed early");
                request.extend_from_slice(buf.get(..n).expect("slice"));
            }
            let split = request
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("head end");
            let head = String::from_utf8_lossy(request.get(..split).expect("head")).to_lowercase();
            let path = head.split_whitespace().nth(1).expect("path").to_owned();
            let (status, reply) = routes
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, s, r)| (*s, r.replace("{BASE}", &own)))
                .expect("route");
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            conn.write_all(response.as_bytes()).expect("write");
            seen.push(Seen {
                path,
                head,
                body: request.get(split + 4..).expect("body").to_vec(),
            });
        }
        seen
    });
    (base, handle)
}

const UPLOAD_ROUTES: &[(&str, &str, &str)] = &[
    (
        "/files.getuploadurlexternal",
        "200 OK",
        r#"{"ok":true,"upload_url":"{BASE}/put/F1","file_id":"F1"}"#,
    ),
    ("/put/f1", "200 OK", "OK - 5"),
    ("/files.completeuploadexternal", "200 OK", r#"{"ok":true}"#),
];

#[test]
fn upload_file_runs_the_three_calls_in_order() {
    let (base, handle) = serve_routes(3, UPLOAD_ROUTES);
    Api::new(&base)
        .expect("api")
        .upload_file(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1730000000.1",
            "proof one.png",
            b"hello",
        )
        .expect("upload");
    let seen = handle.join().expect("server");
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/files.getuploadurlexternal",
            "/put/f1",
            "/files.completeuploadexternal"
        ]
    );
    let [get, put, done] = seen.as_slice() else {
        unreachable!("three requests")
    };
    assert!(get.head.contains(&format!("authorization: bearer {TOKEN}")));
    assert_eq!(
        String::from_utf8_lossy(&get.body),
        "filename=proof%20one.png&length=5"
    );
    assert_eq!(put.body, b"hello");
    assert!(done
        .head
        .contains(&format!("authorization: bearer {TOKEN}")));
    let sent: serde_json::Value = serde_json::from_slice(&done.body).expect("json");
    assert_eq!(
        sent,
        serde_json::json!({
            "files": [{"id": "F1", "title": "proof one.png"}],
            "channel_id": "C1",
            "thread_ts": "1730000000.1",
        })
    );
}

#[test]
fn upload_post_carries_no_authorization_header() {
    let (base, handle) = serve_routes(3, UPLOAD_ROUTES);
    Api::new(&base)
        .expect("api")
        .upload_file(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1.1",
            "a.txt",
            b"hello",
        )
        .expect("upload");
    let seen = handle.join().expect("server");
    let put = seen.get(1).expect("put");
    assert!(!put.head.contains("authorization"), "{}", put.head);
    assert!(!put.head.contains(&TOKEN.to_lowercase()), "{}", put.head);
}

#[test]
fn upload_url_refusal_is_a_slack_error() {
    static ROUTES: &[(&str, &str, &str)] = &[(
        "/files.getuploadurlexternal",
        "200 OK",
        r#"{"ok":false,"error":"missing_scope"}"#,
    )];
    let (base, handle) = serve_routes(1, ROUTES);
    let err = Api::new(&base)
        .expect("api")
        .upload_file(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1.1",
            "a.txt",
            b"x",
        )
        .expect_err("rejected");
    handle.join().expect("server");
    assert!(
        matches!(&err, ApiError::Slack(code) if code == "missing_scope"),
        "{err:?}"
    );
}

#[test]
fn upload_non_2xx_is_a_transport_error_and_skips_completion() {
    static ROUTES: &[(&str, &str, &str)] = &[
        (
            "/files.getuploadurlexternal",
            "200 OK",
            r#"{"ok":true,"upload_url":"{BASE}/put/F1","file_id":"F1"}"#,
        ),
        ("/put/f1", "500 Internal Server Error", "boom"),
    ];
    let (base, handle) = serve_routes(2, ROUTES);
    let err = Api::new(&base)
        .expect("api")
        .upload_file(
            &Secret::new(TOKEN.as_bytes().to_vec()),
            "C1",
            "1.1",
            "a.txt",
            b"x",
        )
        .expect_err("refused");
    handle.join().expect("server");
    assert!(
        matches!(&err, ApiError::Transport(m) if m.contains("500")),
        "{err:?}"
    );
}
