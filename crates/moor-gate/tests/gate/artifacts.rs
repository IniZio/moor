//! Artifact downloads: `GET /boxes/{box}/artifacts/{name}[/{version}]` and the `get_artifact`
//! tool, against a fake controller that lists records and streams bytes frames the way the real
//! one does, refusing missing or mismatched bytes with their detail.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{mpsc, Arc, Mutex};

use moor_core::controller::write_bytes;
use moor_core::response::{ErrorDetail, ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::listen::{post, token};
use super::{world_with, World};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really";
/// Many bytes frames long, and past what the gate holds back before answering.
const BIG: usize = 5 * 1024 * 1024 + 7;

fn sha(tag: char) -> String {
    tag.to_string().repeat(64)
}

/// The fake's records: name, version, content type, sha256, and the bytes when it has them.
fn stored() -> Vec<(&'static str, u32, &'static str, String, Vec<u8>)> {
    let big: Vec<u8> = (0..=250_u8).cycle().take(BIG).collect();
    vec![
        ("notes.txt", 1, "text/plain", sha('1'), b"one".to_vec()),
        ("notes.txt", 2, "text/plain", sha('2'), b"two".to_vec()),
        ("shot.png", 1, "image/png", sha('3'), PNG.to_vec()),
        (
            "big.bin",
            1,
            "application/octet-stream",
            sha('4'),
            big.clone(),
        ),
        ("gone.txt", 1, "text/plain", sha('5'), b"gone".to_vec()),
        ("bad.txt", 1, "text/plain", sha('6'), b"bad".to_vec()),
        ("bad.bin", 1, "application/octet-stream", sha('7'), big),
    ]
}

fn error(kind: ErrorKind, message: &str, detail: Option<ErrorDetail>) -> ErrorView {
    ErrorView {
        kind,
        message: message.to_owned(),
        hints: vec![],
        context: vec![],
        detail,
    }
}

/// Serves `path`; records each request's identity and command.
fn serve_controller(path: &std::path::Path) -> Seen {
    let listener = UnixListener::bind(path).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let shared = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = Arc::clone(&shared);
            std::thread::spawn(move || {
                let mut reply = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["frame"] == "hello" {
                        writeln!(reply, "{line}").unwrap();
                        continue;
                    }
                    let id = frame["id"].as_str().unwrap().to_owned();
                    let inner = &frame["request"]["request"];
                    let cmd = inner["cmd"].as_str().unwrap_or_default().to_owned();
                    let who = frame["request"]["identity"].as_str().unwrap_or_default();
                    seen.lock().unwrap().push((who.to_owned(), cmd.clone()));
                    let envelope = answer(&cmd, inner, &mut reply, &id);
                    let out = json!({"frame": "response", "id": id, "envelope": envelope});
                    writeln!(reply, "{out}").unwrap();
                }
            });
        }
    });
    seen
}

/// The envelope for one request, after any bytes frames it streams.
fn answer(cmd: &str, inner: &Value, reply: &mut impl Write, id: &str) -> Response {
    if cmd == "enroll" {
        return Response::ok(cmd, None);
    }
    if inner["box"] != "repo/a" {
        return Response::err(cmd, error(ErrorKind::NotFound, "no Box", None));
    }
    let records = stored();
    if cmd == "artifacts" {
        let rows: Vec<Value> = records
            .iter()
            .map(|(name, version, kind, sha, bytes)| {
                json!({"name": name, "version": version, "kind": "file", "content_type": kind,
                    "size": bytes.len(), "sha256": sha, "created_at": "2026-11-03T00:00:00Z"})
            })
            .collect();
        return Response::ok(cmd, Some(Value::Array(rows)));
    }
    let wanted = (inner["name"].as_str(), inner["version"].as_u64());
    let Some((name, _, _, sha, bytes)) = records
        .into_iter()
        .find(|r| (Some(r.0), Some(u64::from(r.1))) == wanted)
    else {
        return Response::err(cmd, error(ErrorKind::NotFound, "no artifact", None));
    };
    match name {
        "gone.txt" => {
            let e = error(
                ErrorKind::NotFound,
                "missing on disk",
                Some(ErrorDetail::ArtifactMissing),
            );
            Response::err(cmd, e)
        }
        "bad.txt" | "bad.bin" => {
            write_bytes(reply, id, &bytes).unwrap();
            let why = "artifact does not match its recorded sha256";
            Response::err(
                cmd,
                error(ErrorKind::Failed, why, Some(ErrorDetail::ArtifactMismatch)),
            )
        }
        _ => {
            write_bytes(reply, id, &bytes).unwrap();
            let view = json!({"offset": 0, "sent": bytes.len(), "size": bytes.len(),
                "sha256": sha, "eof": true});
            Response::ok(cmd, Some(view))
        }
    }
}

/// Each request's identity and command, as the fake controller saw them.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn world() -> (World, Seen) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    let (_unused, rx) = mpsc::channel();
    (world_with(socket, rx, dir, true), seen)
}

struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Got {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn error(&self) -> String {
        let doc: Value = serde_json::from_slice(&self.body).unwrap_or(Value::Null);
        doc["error"].as_str().unwrap_or_default().to_owned()
    }
}

fn get(world: &World, token: Option<&str>, path: &str) -> Got {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(format!("http://{}{path}", world.gate));
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let mut res = req.call().unwrap();
    Got {
        status: res.status().as_u16(),
        headers: res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
            .collect(),
        body: res
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .unwrap(),
    }
}

#[test]
fn a_read_token_downloads_the_newest_version_with_its_type_length_and_etag() {
    let (world, seen) = world();
    let token = token(&world, "moor:read");
    let got = get(&world, Some(&token), "/boxes/repo/a/artifacts/notes.txt");
    assert_eq!(got.status, 200, "{}", got.error());
    assert_eq!(got.body, b"two");
    assert_eq!(got.header("content-type"), Some("text/plain"));
    assert_eq!(got.header("content-length"), Some("3"));
    assert_eq!(
        got.header("etag"),
        Some(format!("\"{}\"", sha('2')).as_str())
    );
    let reads: Vec<_> = seen.lock().unwrap().clone();
    assert!(
        reads.contains(&("authgear:alice-sub".to_owned(), "artifact-read".to_owned())),
        "{reads:?}"
    );
}

#[test]
fn a_named_version_is_downloaded_and_the_box_slash_may_be_encoded() {
    let (world, _seen) = world();
    let token = token(&world, "moor:read");
    let got = get(
        &world,
        Some(&token),
        "/boxes/repo%2Fa/artifacts/notes.txt/1",
    );
    assert_eq!((got.status, got.body.as_slice()), (200, b"one".as_slice()));
}

#[test]
fn a_body_larger_than_one_bytes_frame_streams_whole() {
    let (world, _seen) = world();
    let token = token(&world, "moor:read");
    let got = get(&world, Some(&token), "/boxes/repo/a/artifacts/big.bin");
    assert_eq!(got.status, 200);
    assert_eq!(got.body.len(), BIG);
    assert_eq!(got.body, stored()[3].4);
}

#[test]
fn an_unknown_box_name_or_version_is_404() {
    let (world, _seen) = world();
    let token = token(&world, "moor:read");
    for path in [
        "/boxes/repo/zz/artifacts/notes.txt",
        "/boxes/repo/a/artifacts/other.txt",
        "/boxes/repo/a/artifacts/notes.txt/9",
        "/boxes/repo/a",
    ] {
        assert_eq!(get(&world, Some(&token), path).status, 404, "{path}");
    }
}

#[test]
fn bytes_that_are_gone_are_410_and_bytes_that_miss_their_sha256_are_409() {
    let (world, _seen) = world();
    let token = token(&world, "moor:read");
    let gone = get(&world, Some(&token), "/boxes/repo/a/artifacts/gone.txt");
    assert_eq!(gone.status, 410, "{}", gone.error());
    let bad = get(&world, Some(&token), "/boxes/repo/a/artifacts/bad.txt");
    assert_eq!(bad.status, 409);
    assert!(bad.error().contains("sha256"), "{}", bad.error());
}

#[test]
fn a_large_body_that_misses_its_sha256_never_arrives_whole() {
    let (world, _seen) = world();
    let token = token(&world, "moor:read");
    let mut conn = std::net::TcpStream::connect(&world.gate).unwrap();
    write!(
        conn,
        "GET /boxes/repo/a/artifacts/bad.bin HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\r\n"
    )
    .unwrap();
    let mut all = Vec::new();
    std::io::Read::read_to_end(&mut conn, &mut all).unwrap();
    let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = String::from_utf8_lossy(&all[..split]);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains(&format!("Content-Length: {BIG}")), "{head}");
    assert_eq!(all.len() - split, BIG - 1);
}

#[test]
fn no_token_is_401_and_a_token_without_the_read_scope_is_403() {
    let (world, _seen) = world();
    let path = "/boxes/repo/a/artifacts/notes.txt";
    let none = get(&world, None, path);
    assert_eq!(none.status, 401);
    assert!(none.header("www-authenticate").is_some());
    let write = token(&world, "moor:write");
    let got = get(&world, Some(&write), path);
    assert_eq!(got.status, 403);
    assert!(got.error().contains("moor:read"), "{}", got.error());
}

/// The result of calling `get_artifact` with `args`.
fn call(world: &World, scope: &str, args: Value) -> Value {
    let token = token(world, scope);
    let params = json!({"name": "get_artifact", "arguments": args});
    let rx = post(world, &token, "tools/call", params);
    loop {
        let message = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        if let Some(result) = message.get("result") {
            return result.clone();
        }
        assert!(message.get("error").is_none(), "{message}");
    }
}

#[test]
fn the_tool_never_returns_the_bytes_for_any_size() {
    let (world, seen) = world();
    for (name, size) in [("shot.png", PNG.len()), ("notes.txt", 3), ("big.bin", BIG)] {
        let got = call(&world, "moor:read", json!({"box": "repo/a", "name": name}));
        assert_eq!(got["isError"], false, "{got}");
        let blocks = got["content"].as_array().unwrap();
        assert!(
            blocks
                .iter()
                .all(|b| b["type"] == "text" && b["text"].as_str().is_some()),
            "{got}"
        );
        assert!(!got.to_string().contains("\"blob\""), "{name}");
        assert!(!got.to_string().contains("\"data\""), "{name}");
        assert_eq!(got["structuredContent"]["size"], size, "{name}");
    }
    let cmds: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(_, c)| c.clone())
        .collect();
    assert!(!cmds.contains(&"artifact-read".to_owned()), "{cmds:?}");
}

#[test]
fn the_path_the_tool_returns_downloads_the_recorded_bytes() {
    let (world, _seen) = world();
    for (name, want) in [
        ("notes.txt", b"two".to_vec()),
        ("big.bin", stored()[3].4.clone()),
    ] {
        let got = call(&world, "moor:read", json!({"box": "repo/a", "name": name}));
        let record = &got["structuredContent"];
        let path = record["path"].as_str().unwrap();
        let token = token(&world, "moor:read");
        let fetched = get(&world, Some(&token), path);
        assert_eq!(fetched.status, 200, "{}", fetched.error());
        assert_eq!(fetched.body, want, "{name}");
        assert_eq!(
            fetched.header("etag"),
            Some(format!("\"{}\"", record["sha256"].as_str().unwrap()).as_str())
        );
    }
}

#[test]
fn the_tool_needs_the_read_scope() {
    let (world, _seen) = world();
    let refused = call(
        &world,
        "moor:write",
        json!({"box": "repo/a", "name": "notes.txt"}),
    );
    assert_eq!(refused["isError"], true);
    assert!(refused.to_string().contains("moor:read"), "{refused}");
}
