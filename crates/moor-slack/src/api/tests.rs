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
fn team_id_reads_the_team_from_auth_test_with_the_bot_token() {
    let (base, handle) = serve(r#"{"ok":true,"team_id":"T0TEAM1","user_id":"UBOT"}"#);
    let api = Api::new(&base).expect("api");
    let team = api.team_id(&Secret::new(TOKEN.as_bytes().to_vec()));
    assert_eq!(team.expect("auth.test"), "T0TEAM1");
    let request = handle.join().expect("server");
    assert!(request.starts_with("POST /auth.test "), "{request}");
    assert!(
        request.contains(&format!("authorization: Bearer {TOKEN}")),
        "{request}"
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
    assert!(matches!(err, ApiError::RateLimited(None)), "{err:?}");
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
        ApiError::RateLimited(None),
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

mod upload;
