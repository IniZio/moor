//! Tests for the artifact list and download routes, over a fake controller socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::controller::{parse_frame, write_bytes, ClientFrame, BYTES_MAX, PROTOCOL};
use moor_core::response::{ErrorKind, Response};
use serde_json::{json, Value};

use super::super::tests::{
    answers, body, by_command, failure, front, front_with, get, read_value, record, sha, web,
    Responder, Sent, TOKEN,
};
use crate::http::Server;

/// A raw HTTP response: head text and body bytes.
struct Raw {
    head: String,
    body: Vec<u8>,
}

impl Raw {
    fn ok(&self) -> bool {
        self.head.starts_with("HTTP/1.1 200")
    }

    fn status(&self) -> &str {
        self.head.lines().next().unwrap()
    }

    /// Every value of the header `name`, matched case-insensitively.
    fn header(&self, name: &str) -> Vec<&str> {
        self.head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
            .collect()
    }
}

fn fetch(web: &Server, path: &str, token: Option<&str>) -> Raw {
    let addr = web.addr();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = token.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{auth}X-Moor-Csrf: 1\r\n\
         Content-Length: 0\r\n\r\n",
        addr.port()
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    Raw {
        head: String::from_utf8_lossy(&out[..split]).into_owned(),
        body: out[split..].to_vec(),
    }
}

fn download(web: &Server, path: &str) -> Raw {
    fetch(web, path, Some(TOKEN))
}

/// Bytes that are not valid UTF-8, spanning many bytes frames; the last frame holds 100.
fn blob() -> Vec<u8> {
    let size = 2 * 512 * 1024 + 100;
    (0..size)
        .map(|i| u8::try_from(i % 251).unwrap() ^ 0x80)
        .collect()
}

/// A fake controller holding one file `name`, listed with hash `claimed`.
fn holding(
    name: &'static str,
    content_type: &'static str,
    file: Vec<u8>,
    claimed: String,
) -> Responder {
    serving(name, content_type, file.clone(), claimed, move |req| {
        let offset = usize::try_from(req["offset"].as_u64().unwrap()).unwrap();
        let len = usize::try_from(req["len"].as_u64().unwrap()).unwrap_or(usize::MAX);
        let end = offset.saturating_add(len).min(file.len());
        let value = read_value(&file, offset, len);
        (
            file[offset..end].to_vec(),
            Response::ok("artifact-read", Some(value)),
        )
    })
}

/// A fake controller listing one file `name` of `file`'s size with hash `claimed`, whose reads
/// `read` answers.
fn serving(
    name: &'static str,
    content_type: &'static str,
    file: Vec<u8>,
    claimed: String,
    read: impl Fn(&Value) -> (Vec<u8>, Response) + Send + Sync + 'static,
) -> Responder {
    Arc::new(move |req| match req["cmd"].as_str() {
        Some("artifacts") => {
            let mut listed = record(name, content_type, 1, &file);
            listed["sha256"] = json!(claimed);
            (Vec::new(), Response::ok("artifacts", Some(json!([listed]))))
        }
        Some("artifact-read") => read(req),
        other => (
            Vec::new(),
            failure(other.unwrap_or_default(), ErrorKind::Usage),
        ),
    })
}

fn assert_hardened(resp: &Raw) {
    assert!(resp.header("x-content-type-options").contains(&"nosniff"));
    assert!(resp.header("content-security-policy").contains(&"sandbox"));
}

fn reads(sent: &Sent) -> Vec<Value> {
    let sent = sent.lock().unwrap();
    sent.iter()
        .filter(|r| r["cmd"] == "artifact-read")
        .cloned()
        .collect()
}

#[test]
fn artifact_list_is_proxied() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    let resp = get(&web, "/boxes/app/1/artifacts");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let list = body(&resp);
    assert_eq!(list[0]["name"], "note.txt");
    assert_eq!(list[0]["content_type"], "text/plain");
    assert_eq!(list[0]["sha256"], sha(b"hello"));
    assert_eq!(*seen.lock().unwrap(), ["artifacts"]);
}

#[test]
fn artifact_list_passes_the_all_flag() {
    let dir = tempfile::tempdir().unwrap();
    let map = by_command(answers());
    let (web, _, sent) = front_with(&dir, map, &[]);
    assert!(get(&web, "/boxes/app/1/artifacts").starts_with("HTTP/1.1 200"));
    assert!(get(&web, "/boxes/app/1/artifacts?all=1").starts_with("HTTP/1.1 200"));
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0]["box"], "app/1");
    assert_ne!(sent[0]["all"], true);
    assert_eq!(sent[1]["all"], true);
}

#[test]
fn a_binary_artifact_streams_through_in_one_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = blob();
    let respond = holding(
        "out.bin",
        "application/octet-stream",
        file.clone(),
        sha(&file),
    );
    let (web, _, sent) = front_with(&dir, respond, &[]);
    let resp = download(&web, "/boxes/app/1/artifacts/out.bin");
    assert!(resp.ok(), "{}", resp.head);
    assert!(resp.body == file, "body differs from the stored file");
    assert_eq!(resp.header("content-length"), [file.len().to_string()]);
    assert_eq!(resp.header("content-type"), ["application/octet-stream"]);
    assert_eq!(
        resp.header("content-disposition"),
        ["attachment; filename=\"out.bin\""]
    );
    assert_hardened(&resp);
    let whole = json!({"cmd": "artifact-read", "box": "app/1", "name": "out.bin",
                       "version": 1, "offset": 0, "len": file.len()});
    assert_eq!(reads(&sent), [whole]);
}

#[test]
fn images_and_video_are_shown_inline() {
    for (name, content_type) in [("shot.png", "image/png"), ("clip.mp4", "video/mp4")] {
        let dir = tempfile::tempdir().unwrap();
        let file = blob()[..1000].to_vec();
        let respond = holding(name, content_type, file.clone(), sha(&file));
        let (web, _, _) = front_with(&dir, respond, &[]);
        let resp = download(&web, &format!("/boxes/app/1/artifacts/{name}"));
        assert!(resp.ok(), "{}", resp.head);
        assert_eq!(sha(&resp.body), sha(&file));
        assert_eq!(resp.header("content-type"), [content_type]);
        let disposition = resp.header("content-disposition");
        assert!(
            disposition.iter().all(|d| !d.starts_with("attachment")),
            "{disposition:?}"
        );
        assert_hardened(&resp);
    }
}

#[test]
fn the_version_query_selects_that_version() {
    let dir = tempfile::tempdir().unwrap();
    let file = blob()[..1000].to_vec();
    let respond = holding(
        "out.bin",
        "application/octet-stream",
        file.clone(),
        sha(&file),
    );
    let (web, _, sent) = front_with(&dir, respond, &[]);
    assert!(download(&web, "/boxes/app/1/artifacts/out.bin?v=1").ok());
    let reads = reads(&sent);
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert_eq!(reads[0]["version"], 1);
    let sent = sent.lock().unwrap().clone();
    assert!(
        sent.iter()
            .filter(|r| r["cmd"] == "artifacts")
            .all(|r| r["all"] == true),
        "{sent:?}"
    );
}

/// Downloads `out.bin`, listed as `size` bytes with hash `listed`, from a controller that
/// sends `served` and then `envelope`.
fn download_served(size: usize, listed: String, served: Vec<u8>, envelope: Response) -> Raw {
    let dir = tempfile::tempdir().unwrap();
    let respond = serving(
        "out.bin",
        "application/octet-stream",
        vec![0; size],
        listed,
        move |_| (served.clone(), envelope.clone()),
    );
    let (web, _, _) = front_with(&dir, respond, &[]);
    download(&web, "/boxes/app/1/artifacts/out.bin")
}

fn served_ok(file: &[u8]) -> Response {
    Response::ok("artifact-read", Some(read_value(file, 0, file.len())))
}

#[test]
fn a_hash_mismatch_never_delivers_the_whole_file() {
    for file in [blob()[..1000].to_vec(), blob()] {
        let resp = download_served(file.len(), "0".repeat(64), file.clone(), served_ok(&file));
        assert_eq!(resp.header("content-length"), [file.len().to_string()]);
        assert!(resp.body.len() < file.len(), "{} bytes", resp.body.len());
        assert!(file.starts_with(&resp.body));
    }
}

#[test]
fn a_controller_failure_after_the_bytes_never_delivers_the_whole_file() {
    let file = blob();
    let failed = failure("artifact-read", ErrorKind::Failed);
    let resp = download_served(file.len(), sha(&file), file.clone(), failed);
    assert!(resp.ok(), "{}", resp.head);
    assert!(resp.body.len() < file.len(), "{} bytes", resp.body.len());
}

#[test]
fn a_file_shorter_than_its_record_is_502() {
    let resp = download_served(1000, sha(b""), Vec::new(), served_ok(b""));
    assert!(resp.status().starts_with("HTTP/1.1 502"), "{}", resp.head);
}

/// A controller that lists `big.bin` of `size` bytes and streams it to any reader, counting
/// the bytes it has handed to the socket.
fn streaming_controller(sock: &Path, size: u64) -> Arc<AtomicU64> {
    let listener = UnixListener::bind(sock).unwrap();
    let written = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&written);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut reader = BufReader::new(stream.unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut out = reader.get_ref().try_clone().unwrap();
            writeln!(out, r#"{{"frame":"hello","protocol":{PROTOCOL}}}"#).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let Ok(ClientFrame::Request { id, request }) = parse_frame(line.trim_end()) else {
                continue;
            };
            let value = if request.cmd() == "artifacts" {
                json!([{"name": "big.bin", "version": 1, "kind": "file",
                        "content_type": "application/octet-stream", "size": size,
                        "sha256": "0".repeat(64), "created_at": "2026-01-01T00:00:00Z"}])
            } else {
                let piece = vec![7u8; BYTES_MAX];
                let mut left = size;
                while left > 0 {
                    let n = usize::try_from(left.min(BYTES_MAX as u64)).unwrap();
                    if write_bytes(&mut out, &id, &piece[..n]).is_err() {
                        break;
                    }
                    count.fetch_add(n as u64, Ordering::SeqCst);
                    left -= n as u64;
                }
                Value::Null
            };
            let envelope = Response::ok(request.cmd(), Some(value));
            let frame = json!({"frame": "response", "id": id, "envelope": envelope});
            let _ = writeln!(out, "{frame}");
        }
    });
    written
}

#[test]
fn a_download_holds_a_bounded_number_of_bytes_while_the_browser_waits() {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let written = streaming_controller(&sock, 4 * LIMIT);
    let web = web(&dir, &sock, &[]);
    let mut browser = TcpStream::connect(web.addr()).unwrap();
    write!(
        browser,
        "GET /boxes/app/1/artifacts/big.bin HTTP/1.1\r\nHost: x\r\n\
         Authorization: Bearer {TOKEN}\r\n\r\n"
    )
    .unwrap();
    let mut last = u64::MAX;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(200));
        let now = written.load(Ordering::SeqCst);
        if now == last {
            break;
        }
        last = now;
    }
    assert!(last > 0, "the controller sent nothing");
    assert!(
        last <= LIMIT,
        "{last} bytes left the controller with none read"
    );
}

#[test]
fn an_unknown_artifact_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let mut map = (*answers()).clone();
    map.insert(
        "artifact-read",
        failure("artifact-read", ErrorKind::NotFound),
    );
    let (web, _) = front(&dir, Arc::new(map), &[]);
    let resp = download(&web, "/boxes/app/1/artifacts/note.txt");
    assert!(resp.status().starts_with("HTTP/1.1 404"), "{}", resp.head);
}

#[test]
fn bad_artifact_requests_are_400_and_never_reach_the_controller() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    for path in [
        "/boxes/Bad%20Name/artifacts",
        "/boxes/Bad%20Name/artifacts/note.txt",
        "/boxes/app/1/artifacts/..%2Fx",
        "/boxes/app/1/artifacts/.hidden",
        "/boxes/app/1/artifacts/note.txt?v=x",
        "/boxes/app/1/artifacts/note.txt?v=-1",
    ] {
        let resp = download(&web, path);
        assert!(
            resp.status().starts_with("HTTP/1.1 400"),
            "{path}: {}",
            resp.head
        );
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "{:?}",
        seen.lock().unwrap()
    );
}

#[test]
fn artifact_routes_need_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let (web, seen) = front(&dir, answers(), &[]);
    for path in ["/boxes/app/1/artifacts", "/boxes/app/1/artifacts/note.txt"] {
        for token in [None, Some("nope")] {
            let resp = fetch(&web, path, token);
            assert!(
                resp.status().starts_with("HTTP/1.1 401"),
                "{path}: {}",
                resp.head
            );
        }
    }
    assert!(seen.lock().unwrap().is_empty());
}
