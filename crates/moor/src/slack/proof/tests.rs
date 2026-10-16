//! Tests for proof posting against a fake Slack on loopback and a real store.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built values"
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::TaskName;
use serde_json::{json, Value};

use super::*;
use crate::testing::clock::FakeClock;

/// A token whose text must never show up in a message.
const TOKEN: &str = "xoxb-proof-secret";

/// Opens a store in `dir`.
fn store(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// One request the fake Slack saw.
struct Seen {
    /// Request path.
    path: String,
    /// Request body.
    body: Vec<u8>,
}

/// A fake Slack that answers by path and records every request.
struct Fake {
    /// Base URL of the Web API.
    base: String,
    /// Requests in arrival order.
    seen: Arc<Mutex<Vec<Seen>>>,
    /// While set, the file upload itself answers HTTP 500.
    upload_down: Arc<AtomicBool>,
    /// While set, `chat.postMessage` answers `ok: false`.
    post_down: Arc<AtomicBool>,
}

/// Index of the end of the headers, when they have fully arrived.
fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Reads one request: path and body.
fn read_request(conn: &mut std::net::TcpStream) -> Option<Seen> {
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if let Some(end) = head_end(&buf) {
            let head = String::from_utf8_lossy(buf.get(..end)?).to_string();
            let want = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buf.len() >= end + want {
                let path = head.split_whitespace().nth(1)?.to_owned();
                return Some(Seen {
                    path,
                    body: buf.get(end..end + want)?.to_vec(),
                });
            }
        }
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(chunk.get(..n)?);
    }
}

/// Starts the fake on a loopback port; it runs until the test process ends.
fn fake() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let upload_down = Arc::new(AtomicBool::new(false));
    let post_down = Arc::new(AtomicBool::new(false));
    let (s, u, p, h) = (
        seen.clone(),
        upload_down.clone(),
        post_down.clone(),
        host.clone(),
    );
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Some(req) = read_request(&mut conn) else {
                continue;
            };
            let (status, reply) = if req.path.contains("getUploadURLExternal") {
                let url = format!("{h}/upload/F1");
                (
                    "200 OK",
                    json!({"ok": true, "upload_url": url, "file_id": "F1"}).to_string(),
                )
            } else if req.path.starts_with("/upload/") {
                if u.load(Ordering::SeqCst) {
                    ("500 Internal Server Error", "boom".to_owned())
                } else {
                    ("200 OK", "OK - 5".to_owned())
                }
            } else if req.path.contains("completeUploadExternal") {
                (
                    "200 OK",
                    json!({"ok": true, "files": [{"id": "F1"}]}).to_string(),
                )
            } else if p.load(Ordering::SeqCst) {
                (
                    "200 OK",
                    json!({"ok": false, "error": "fatal_error"}).to_string(),
                )
            } else {
                ("200 OK", json!({"ok": true, "ts": "9.9"}).to_string())
            };
            s.lock().unwrap().push(req);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = conn.write_all(response.as_bytes());
        }
    });
    Fake {
        base: format!("{host}/api"),
        seen,
        upload_down,
        post_down,
    }
}

impl Fake {
    /// A client for this fake.
    fn api(&self) -> Api {
        Api::new(&self.base).unwrap()
    }

    /// Paths seen so far.
    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.path.clone())
            .collect()
    }

    /// Bodies of requests whose path contains `part`, as lossy text.
    fn bodies(&self, part: &str) -> Vec<String> {
        let hits = |s: &&Seen| s.path.contains(part);
        let seen = self.seen.lock().unwrap();
        let out: Vec<String> = seen
            .iter()
            .filter(hits)
            .map(|s| String::from_utf8_lossy(&s.body).into_owned())
            .collect();
        drop(seen);
        out
    }

    /// Texts posted with `chat.postMessage`.
    fn texts(&self) -> Vec<String> {
        let posts = self.bodies("chat.postMessage");
        let parse = |b: &String| serde_json::from_str::<Value>(b).unwrap();
        posts
            .iter()
            .map(|b| parse(b)["text"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Number of file uploads that reached the upload URL.
    fn uploads(&self) -> usize {
        self.bodies("/upload/").len()
    }
}

/// A thread for task `t1`.
fn threads() -> Vec<SlackThread> {
    vec![SlackThread {
        channel: "C1".into(),
        thread_ts: "10.5".into(),
        task: TaskName::parse("t1").unwrap(),
        owner: "U1".into(),
        status_ts: None,
    }]
}

/// Writes the controller's copy of `name` for task `t1`.
fn write_copy(state: &Path, name: &str, bytes: &[u8]) {
    let path = state.join(copy_relpath("t1", name));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A `proof` payload.
fn payload(name: &str, sha: &str, refused: Option<&str>) -> Value {
    let mut p = json!({"name": name, "size": 5, "sha256": sha});
    if let Some(r) = refused {
        p["refused"] = json!(r);
    }
    p
}

/// Folds `payload` and posts what is due with a fresh queue.
fn run(f: &Fake, store: &Store, state: &Path, proofs: &mut Proofs, payload: &Value) {
    proofs.fold("t1", "proof", payload);
    let bot = Secret::new(TOKEN.as_bytes().to_vec());
    let log = |m: &str| assert!(!m.contains(TOKEN), "token leaked: {m}");
    proofs.post_due(store, &f.api(), &bot, &threads(), state, &log);
}

#[test]
fn uploads_the_copy_into_the_thread() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    write_copy(dir.path(), "out.txt", b"hello");
    let st = store(dir.path());
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("out.txt", "aa", None),
    );
    assert_eq!(f.bodies("/upload/"), vec!["hello".to_owned()]);
    let done = f.bodies("completeUploadExternal").join("");
    assert!(done.contains("C1") && done.contains("10.5"), "{done}");
    assert!(f.texts().is_empty());
    assert!(st.slack_proof_posted("t1", "out.txt", "aa").unwrap());
}

#[test]
fn refused_proof_posts_text_and_no_upload() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let st = store(dir.path());
    let p = payload("big.bin", "bb", Some("larger than the cap"));
    run(&f, &st, dir.path(), &mut Proofs::default(), &p);
    assert_eq!(
        f.texts(),
        vec!["Proof big.bin not uploaded: larger than the cap"]
    );
    assert!(f.paths().iter().all(|p| p.contains("chat.postMessage")));
    assert!(st.slack_proof_posted("t1", "big.bin", "bb").unwrap());
}

#[test]
fn missing_copy_posts_failure_text() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let st = store(dir.path());
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("gone.txt", "cc", None),
    );
    let texts = f.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].starts_with("Proof gone.txt could not be uploaded: "),
        "{texts:?}"
    );
    assert_eq!(f.uploads(), 0);
    assert!(st.slack_proof_posted("t1", "gone.txt", "cc").unwrap());
}

#[test]
fn failed_upload_posts_failure_text_without_the_token() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    write_copy(dir.path(), "out.txt", b"hello");
    f.upload_down.store(true, Ordering::SeqCst);
    let st = store(dir.path());
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("out.txt", "aa", None),
    );
    let texts = f.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].starts_with("Proof out.txt could not be uploaded: "),
        "{texts:?}"
    );
    assert!(!texts[0].contains(TOKEN));
    assert!(st.slack_proof_posted("t1", "out.txt", "aa").unwrap());
}

#[test]
fn restart_does_not_post_again_but_a_new_sha_does() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    write_copy(dir.path(), "out.txt", b"hello");
    let st = store(dir.path());
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("out.txt", "aa", None),
    );
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("out.txt", "aa", None),
    );
    assert_eq!(f.uploads(), 1);
    write_copy(dir.path(), "out.txt", b"world");
    run(
        &f,
        &st,
        dir.path(),
        &mut Proofs::default(),
        &payload("out.txt", "dd", None),
    );
    assert_eq!(
        f.bodies("/upload/"),
        vec!["hello".to_owned(), "world".to_owned()]
    );
}

#[test]
fn text_post_failure_keeps_it_queued() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.post_down.store(true, Ordering::SeqCst);
    let st = store(dir.path());
    let mut proofs = Proofs::default();
    let p = payload("big.bin", "bb", Some("too big"));
    run(&f, &st, dir.path(), &mut proofs, &p);
    assert!(!st.slack_proof_posted("t1", "big.bin", "bb").unwrap());
    f.post_down.store(false, Ordering::SeqCst);
    let bot = Secret::new(TOKEN.as_bytes().to_vec());
    proofs.post_due(&st, &f.api(), &bot, &threads(), dir.path(), &|_| {});
    assert!(st.slack_proof_posted("t1", "big.bin", "bb").unwrap());
    assert_eq!(f.texts().len(), 2);
}

#[test]
fn proof_without_thread_stays_queued_and_other_kinds_are_ignored() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    write_copy(dir.path(), "out.txt", b"hello");
    let st = store(dir.path());
    let mut proofs = Proofs::default();
    proofs.fold("t1", "working", &payload("out.txt", "aa", None));
    proofs.fold("t1", "proof", &payload("out.txt", "aa", None));
    let bot = Secret::new(TOKEN.as_bytes().to_vec());
    proofs.post_due(&st, &f.api(), &bot, &[], dir.path(), &|_| {});
    assert!(f.paths().is_empty());
    proofs.post_due(&st, &f.api(), &bot, &threads(), dir.path(), &|_| {});
    assert_eq!(f.uploads(), 1);
}
