//! Tests for posting an artifact version, against a fake Slack and a fake controller socket.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built values"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::controller::{ServerFrame, BYTES_MAX, PROTOCOL};
use moor_core::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::*;

/// A token whose text must never reach a thread.
const TOKEN: &str = "xoxb-artifact-secret";
/// Bodies up to this size are kept whole; longer ones are only counted.
const KEEP: u64 = 1 << 20;

/// One request the fake Slack saw.
struct Seen {
    /// Request path.
    path: String,
    /// Body length.
    len: u64,
    /// Body, when it is at most [`KEEP`] bytes.
    body: Vec<u8>,
}

/// A fake Slack that answers by path and records every request.
pub(crate) struct Fake {
    /// Base URL of the Web API.
    pub(crate) base: String,
    /// Requests in arrival order.
    seen: Mutex<Vec<Seen>>,
    /// While set, the file POST answers HTTP 500 with the token in its body.
    pub(crate) upload_down: AtomicBool,
    /// While set, `files.getUploadURLExternal` answers `missing_scope`.
    scope_missing: AtomicBool,
    /// While set, `chat.postMessage` answers `ok: false`.
    post_down: AtomicBool,
    /// When set, the upload URL `files.getUploadURLExternal` hands out.
    upload_to: Mutex<Option<String>>,
}

/// Reads one request, keeping at most [`KEEP`] body bytes.
fn read_request(conn: &TcpStream) -> Option<Seen> {
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(conn);
    let (mut path, mut len) = (None, 0_u64);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end().to_ascii_lowercase();
        if line.is_empty() {
            break;
        }
        path = path.or_else(|| line.split_whitespace().nth(1).map(str::to_owned));
        if let Some(v) = line.strip_prefix("content-length:") {
            len = v.trim().parse().ok()?;
        }
    }
    let (mut body, mut chunk, mut left) = (Vec::new(), vec![0_u8; 64 * 1024], len);
    while left > 0 {
        let want = chunk.len().min(usize::try_from(left).ok()?);
        let n = reader.read(&mut chunk[..want]).ok()?;
        if n == 0 {
            return None;
        }
        if len <= KEEP {
            body.extend_from_slice(&chunk[..n]);
        }
        left -= n as u64;
    }
    Some(Seen {
        path: path?,
        len,
        body,
    })
}

/// The fake's status and reply for `path`.
fn reply(fake: &Fake, host: &str, path: &str) -> (&'static str, String) {
    let on = |flag: &AtomicBool| flag.load(Ordering::SeqCst);
    if path.ends_with("getuploadurlexternal") {
        if on(&fake.scope_missing) {
            return (
                "200 OK",
                json!({"ok": false, "error": "missing_scope"}).to_string(),
            );
        }
        let url = fake
            .upload_to
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| format!("{host}/upload/F1"));
        let reply = json!({"ok": true, "upload_url": url, "file_id": "F1"});
        return ("200 OK", reply.to_string());
    }
    if path.starts_with("/upload/") && on(&fake.upload_down) {
        return (
            "500 Internal Server Error",
            format!("bad auth Bearer {TOKEN}"),
        );
    }
    if path.ends_with("chat.postmessage") && on(&fake.post_down) {
        return (
            "200 OK",
            json!({"ok": false, "error": "fatal_error"}).to_string(),
        );
    }
    ("200 OK", json!({"ok": true, "ts": "9.9"}).to_string())
}

/// Starts the fake on a loopback port; it runs until the test process ends.
pub(crate) fn fake() -> Arc<Fake> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let fake = Arc::new(Fake {
        base: format!("{host}/api"),
        seen: Mutex::default(),
        upload_down: AtomicBool::default(),
        scope_missing: AtomicBool::default(),
        post_down: AtomicBool::default(),
        upload_to: Mutex::default(),
    });
    let served = Arc::clone(&fake);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Some(req) = read_request(&conn) else {
                continue;
            };
            let (status, body) = reply(&served, &host, &req.path);
            served.seen.lock().unwrap().push(req);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = conn.write_all(response.as_bytes());
        }
    });
    fake
}

impl Fake {
    /// Requests whose path contains `part`, in arrival order: (length, body).
    pub(crate) fn hits(&self, part: &str) -> Vec<(u64, Vec<u8>)> {
        let seen = self.seen.lock().unwrap();
        let hits = seen
            .iter()
            .filter(|s| s.path.contains(part))
            .map(|s| (s.len, s.body.clone()))
            .collect();
        drop(seen);
        hits
    }

    /// Texts posted with `chat.postMessage`.
    pub(crate) fn texts(&self) -> Vec<String> {
        let text = |(_, b): &(u64, Vec<u8>)| {
            let v: Value = serde_json::from_slice(b).unwrap();
            v["text"].as_str().unwrap().to_owned()
        };
        self.hits("chat.postmessage").iter().map(text).collect()
    }
}

/// What the fake controller streams for one connection: `pattern` repeated up to `total`
/// bytes, in bytes frames of `frame` bytes, then an ok response or the error `fail`.
pub(crate) struct Stored {
    /// Bytes repeated to make the file.
    pattern: Vec<u8>,
    /// File length.
    total: u64,
    /// Bytes per frame.
    frame: usize,
    /// Error message of the final response, if it fails.
    fail: Option<&'static str>,
}

impl Stored {
    /// `bytes` as one file in full frames.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self {
            pattern: bytes.to_vec(),
            total: bytes.len() as u64,
            frame: BYTES_MAX,
            fail: None,
        }
    }
}

/// Writes the frames of `s` to `out`.
fn send(out: &mut impl Write, s: &Stored) -> std::io::Result<()> {
    let (mut at, mut sent, mut frame) = (0, 0_u64, Vec::with_capacity(s.frame));
    while sent < s.total {
        let want = s.frame.min(usize::try_from(s.total - sent).unwrap());
        frame.clear();
        while frame.len() < want {
            let take = (want - frame.len()).min(s.pattern.len() - at);
            frame.extend_from_slice(&s.pattern[at..at + take]);
            at = (at + take) % s.pattern.len();
        }
        let head = ServerFrame::Bytes {
            id: "1".into(),
            len: want,
        };
        out.write_all(head.to_line().as_bytes())?;
        out.write_all(&frame)?;
        sent += want as u64;
    }
    let envelope = s.fail.map_or_else(
        || Response::ok("artifact-read", Some(json!({"sent": sent, "eof": true}))),
        |message| {
            let error = ErrorView {
                kind: ErrorKind::Failed,
                message: message.to_owned(),
                hints: Vec::new(),
                context: Vec::new(),
            };
            Response::err("artifact-read", error)
        },
    );
    let end = ServerFrame::Response {
        id: "1".into(),
        envelope,
    };
    out.write_all(end.to_line().as_bytes())
}

/// Answers the hello on `conn` and returns the request that follows it.
fn take_request(mut conn: &UnixStream) -> Value {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello = ServerFrame::Hello { protocol: PROTOCOL };
    conn.write_all(hello.to_line().as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str::<Value>(&line).unwrap()["request"].clone()
}

/// Serves one connection per entry of `answers` on `sock`, then records any request that
/// arrives within 300 ms and answers none. Yields each request received.
pub(crate) fn controller(sock: &Path, answers: Vec<Stored>) -> JoinHandle<Vec<Value>> {
    let listener = UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for stored in answers {
            let (mut conn, _) = listener.accept().unwrap();
            seen.push(take_request(&conn));
            let _ = send(&mut conn, &stored);
        }
        listener.set_nonblocking(true).unwrap();
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(50));
            if let Ok((conn, _)) = listener.accept() {
                conn.set_nonblocking(false).unwrap();
                seen.push(take_request(&conn));
            }
        }
        seen
    })
}

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Version 2 of `name`, listed with `size` and `sha256`.
fn listed(name: &str, size: u64, sha256: String) -> ArtifactView {
    ArtifactView {
        name: name.to_owned(),
        version: 2,
        kind: "file".into(),
        content_type: "application/octet-stream".into(),
        size,
        sha256,
        created_at: "2026-10-21T00:00:00Z".into(),
    }
}

/// Posts `v` of Box `t1` into thread `10.5` of `C1` through Slack at `base`.
fn post(base: &str, sock: &Path, v: &ArtifactView) -> Result<(), ApiError> {
    let (api, bot) = (
        Api::new(base).unwrap(),
        Secret::new(TOKEN.as_bytes().to_vec()),
    );
    let thread = Thread {
        api: &api,
        bot: &bot,
        channel: "C1",
        ts: "10.5",
    };
    let box_name = BoxName::parse_allow_bare("t1").unwrap();
    post_version(&thread, &Client::new(sock), &box_name, v, false).map(drop)
}

#[test]
fn uploads_the_stored_bytes_and_shares_them_in_the_thread() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"hello"))).unwrap();
    let asked = ctl.join().unwrap();
    assert_eq!(
        asked,
        [
            json!({"cmd": "artifact-read", "box": "t1", "name": "report.txt", "version": 2, "offset": 0, "len": 5})
        ]
    );
    assert_eq!(f.hits("/upload/"), [(5, b"hello".to_vec())]);
    let done: Value = serde_json::from_slice(&f.hits("completeuploadexternal")[0].1).unwrap();
    assert_eq!(
        done,
        json!({"files": [{"id": "F1", "title": "report.txt"}], "channel_id": "C1", "thread_ts": "10.5"})
    );
    assert!(f.texts().is_empty());
}

#[test]
fn every_byte_value_arrives_unchanged() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let pattern: Vec<u8> = (0..=255).collect();
    let total = 3 * 4093 + 77;
    let source: Vec<u8> = pattern.iter().copied().cycle().take(total).collect();
    let stored = Stored {
        pattern,
        total: total as u64,
        frame: 4093,
        fail: None,
    };
    let ctl = controller(&sock, vec![stored]);
    post(
        &f.base,
        &sock,
        &listed("all.bin", total as u64, sha(&source)),
    )
    .unwrap();
    ctl.join().unwrap();
    assert!(
        f.hits("/upload/")[0].1 == source,
        "the body differs from the stored bytes"
    );
    assert_eq!(f.hits("completeuploadexternal").len(), 1);
    assert!(f.texts().is_empty(), "{:?}", f.texts());
}

#[test]
fn a_checksum_mismatch_never_shares_the_file() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hellp")]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"hello"))).unwrap();
    ctl.join().unwrap();
    assert!(f.hits("completeuploadexternal").is_empty());
    assert_eq!(
        f.texts(),
        ["report.txt could not be uploaded: the stored copy does not match its checksum"]
    );
}

#[test]
fn a_read_error_after_bytes_never_shares_the_file() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let stored = Stored {
        fail: Some("artifact report.txt vanished"),
        ..Stored::of(b"abc")
    };
    let ctl = controller(&sock, vec![stored]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"abcde"))).unwrap();
    ctl.join().unwrap();
    assert!(f.hits("completeuploadexternal").is_empty());
    assert_eq!(
        f.texts(),
        ["report.txt could not be uploaded: cannot read the stored copy: artifact report.txt vanished"]
    );
}

#[test]
fn a_short_stored_copy_never_shares_the_file() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"abc")]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"abcde"))).unwrap();
    ctl.join().unwrap();
    assert!(f.hits("completeuploadexternal").is_empty());
    assert_eq!(
        f.texts(),
        ["report.txt could not be uploaded: the stored copy is shorter than its listed size"]
    );
}

/// Asserts that a stored copy longer than the listed 5 bytes is never shared.
fn longer_copy_is_refused(stored: Stored) {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![stored]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"abcde"))).unwrap();
    ctl.join().unwrap();
    assert!(f.hits("completeuploadexternal").is_empty());
    assert_eq!(
        f.texts(),
        ["report.txt could not be uploaded: the stored copy is longer than its listed size"]
    );
}

#[test]
fn a_longer_stored_copy_in_the_last_frame_never_shares_the_file() {
    longer_copy_is_refused(Stored::of(b"abcdefgh"));
}

#[test]
fn a_longer_stored_copy_in_later_frames_never_shares_the_file() {
    longer_copy_is_refused(Stored {
        frame: 5,
        ..Stored::of(b"abcdefghij")
    });
}

#[test]
fn the_upload_url_never_reaches_an_error_or_the_thread() {
    const MARKER: &str = "capability-marker-7f3a";
    let gone = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload/{MARKER}", gone.local_addr().unwrap());
    drop(gone);
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    *f.upload_to.lock().unwrap() = Some(url);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"hello"))).unwrap();
    ctl.join().unwrap();
    let texts = f.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].starts_with("report.txt could not be uploaded: "),
        "{texts:?}"
    );
    assert!(!texts[0].contains(MARKER), "{texts:?}");
    assert!(f.hits("completeuploadexternal").is_empty());
}

#[test]
fn missing_scope_posts_one_name_and_size_line() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.scope_missing.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, Vec::new());
    post(&f.base, &sock, &listed("<r>.txt", 2048, sha(&[b'x'; 2048]))).unwrap();
    assert!(ctl.join().unwrap().is_empty(), "nothing was read");
    assert_eq!(f.texts(), ["&lt;r&gt;.txt (2.0 KB)"]);
    assert!(f.hits("/upload/").is_empty());
}

#[test]
fn a_refused_upload_posts_the_reason_without_the_token() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.upload_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    post(&f.base, &sock, &listed("report.txt", 5, sha(b"hello"))).unwrap();
    let asked = ctl.join().unwrap();
    assert_eq!(asked.len(), 1, "the front only read the Box: {asked:?}");
    assert_eq!(asked[0]["cmd"], "artifact-read");
    let texts = f.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].starts_with("report.txt could not be uploaded: ") && texts[0].contains("500"),
        "{texts:?}"
    );
    assert!(!texts[0].contains(TOKEN), "{texts:?}");
    assert!(f.hits("completeuploadexternal").is_empty());
}

#[test]
fn an_unposted_text_is_an_error_so_the_caller_retries() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.scope_missing.store(true, Ordering::SeqCst);
    f.post_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, Vec::new());
    let err = post(&f.base, &sock, &listed("report.txt", 5, sha(b"hello"))).unwrap_err();
    ctl.join().unwrap();
    assert!(
        matches!(&err, ApiError::Slack(code) if code == "fatal_error"),
        "{err:?}"
    );
}

#[test]
fn size_text_matches_the_web_view() {
    let kib = 1024;
    let cases = [
        (0, "0 B"),
        (1023, "1023 B"),
        (kib, "1.0 KB"),
        (1536, "1.5 KB"),
        (10 * kib, "10 KB"),
        (1023 * kib, "1023 KB"),
        (5 * kib * kib, "5.0 MB"),
        (300 * kib * kib, "300 MB"),
        (3 * kib * kib * kib, "3.0 GB"),
        (2048 * kib * kib * kib, "2048 GB"),
    ];
    for (n, want) in cases {
        assert_eq!(size_text(n), want, "{n} bytes");
    }
}

/// Size of the large upload.
const BIG: u64 = 256 << 20;
/// Most the uploading process's peak RSS may grow over its baseline.
const GROWTH: u64 = 16 << 20;
/// Environment variables that hand the fakes to [`upload_in_a_child`].
const CHILD_ENV: [&str; 2] = ["MOOR_SLACK_TEST_SOCKET", "MOOR_SLACK_TEST_API"];

/// Peak resident set size of this process, in bytes.
fn peak_rss() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    let kib: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib * 1024
}

#[test]
fn a_large_upload_keeps_peak_memory_flat() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let big = Stored {
        pattern: (0..=250_u8).cycle().take(BYTES_MAX).collect(),
        total: BIG,
        frame: BYTES_MAX,
        fail: None,
    };
    let ctl = controller(&sock, vec![Stored::of(&[b'x'; 1024]), big]);
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["upload::tests::upload_in_a_child", "--exact", "--ignored"])
        .env(CHILD_ENV[0], &sock)
        .env(CHILD_ENV[1], &f.base)
        .output()
        .unwrap();
    ctl.join().unwrap();
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{log}");
    let sizes: Vec<u64> = f.hits("/upload/").iter().map(|(n, _)| *n).collect();
    assert_eq!(sizes, [1024, BIG], "{log}");
}

#[test]
#[ignore = "a_large_upload_keeps_peak_memory_flat runs it in a process of its own"]
fn upload_in_a_child() {
    let [Some(sock), Some(base)] = CHILD_ENV.map(std::env::var_os) else {
        return;
    };
    let base = base.to_str().unwrap();
    let sock = Path::new(&sock);
    post(base, sock, &listed("small.bin", 1024, sha(&[b'x'; 1024]))).unwrap();
    let before = peak_rss();
    post(base, sock, &listed("big.bin", BIG, "0".repeat(64))).unwrap();
    let growth = peak_rss().saturating_sub(before);
    assert!(growth <= GROWTH, "peak RSS grew {growth} bytes");
}
