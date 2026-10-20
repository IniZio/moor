//! Tests for showing artifact versions in a thread, against a fake Slack and a real store.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built values"
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName};
use serde_json::{json, Value};

use super::*;
use crate::slack::status::Feed;
use crate::testing::clock::FakeClock;

/// A token whose text must never reach a thread.
const TOKEN: &str = "xoxb-artifact-secret";

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
    /// While set, the file POST answers HTTP 500 with the token in its body.
    upload_down: Arc<AtomicBool>,
    /// While set, `files.getUploadURLExternal` answers `missing_scope`.
    scope_missing: Arc<AtomicBool>,
    /// While set, `chat.postMessage` answers `ok: false`.
    post_down: Arc<AtomicBool>,
}

/// Reads one request: path and body.
fn read_request(conn: &mut TcpStream) -> Option<Seen> {
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4) {
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
                let body = buf.get(end..end + want)?.to_vec();
                return Some(Seen { path, body });
            }
        }
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(chunk.get(..n)?);
    }
}

/// The fake's status and reply for `path`.
fn reply(fake: &Fake, host: &str, path: &str) -> (&'static str, String) {
    let on = |flag: &AtomicBool| flag.load(Ordering::SeqCst);
    if path.contains("getUploadURLExternal") {
        if on(&fake.scope_missing) {
            return (
                "200 OK",
                json!({"ok": false, "error": "missing_scope"}).to_string(),
            );
        }
        let url = format!("{host}/upload/F1");
        return (
            "200 OK",
            json!({"ok": true, "upload_url": url, "file_id": "F1"}).to_string(),
        );
    }
    if path.starts_with("/upload/") {
        if on(&fake.upload_down) {
            return (
                "500 Internal Server Error",
                format!("bad auth Bearer {TOKEN}"),
            );
        }
        return ("200 OK", "OK".to_owned());
    }
    if path.contains("chat.postMessage") && on(&fake.post_down) {
        return (
            "200 OK",
            json!({"ok": false, "error": "fatal_error"}).to_string(),
        );
    }
    ("200 OK", json!({"ok": true, "ts": "9.9"}).to_string())
}

/// Starts the fake on a loopback port; it runs until the test process ends.
fn fake() -> Arc<Fake> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let fake = Arc::new(Fake {
        base: format!("{host}/api"),
        seen: Arc::default(),
        upload_down: Arc::default(),
        scope_missing: Arc::default(),
        post_down: Arc::default(),
    });
    let served = Arc::clone(&fake);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let Some(req) = read_request(&mut conn) else {
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
    /// A client for this fake.
    fn api(&self) -> Api {
        Api::new(&self.base).unwrap()
    }

    /// Paths seen so far.
    fn paths(&self) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        seen.iter().map(|s| s.path.clone()).collect()
    }

    /// Bodies of requests whose path contains `part`.
    fn bodies(&self, part: &str) -> Vec<Vec<u8>> {
        let seen = self.seen.lock().unwrap();
        let hits: Vec<Vec<u8>> = seen
            .iter()
            .filter(|s| s.path.contains(part))
            .map(|s| s.body.clone())
            .collect();
        drop(seen);
        hits
    }

    /// Texts posted with `chat.postMessage`.
    fn texts(&self) -> Vec<String> {
        let posts = self.bodies("chat.postMessage");
        let text = |b: &Vec<u8>| {
            let v: Value = serde_json::from_slice(b).unwrap();
            v.get("text").and_then(Value::as_str).unwrap().to_owned()
        };
        posts.iter().map(text).collect()
    }
}

/// A store in `dir` with one open Box named `t1`.
fn store_with_box(dir: &Path) -> (Store, BoxId) {
    let store = open_store(dir);
    let id = add_box(&store, "t1");
    (store, id)
}

/// Opens the store in `dir`, as a controller process does on start.
fn open_store(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

/// Inserts an open Box named `name`.
fn add_box(store: &Store, name: &str) -> BoxId {
    store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id
}

/// Where the tests keep artifact files.
fn artifacts_dir(dir: &Path) -> PathBuf {
    dir.join("artifacts")
}

/// Stores `bytes` as the next version of `name` and returns that version.
fn publish(store: &Store, dir: &Path, id: BoxId, name: &str, bytes: &[u8]) -> Published {
    let row = store
        .add_artifact(id, name, bytes, &artifacts_dir(dir))
        .unwrap()
        .unwrap();
    Published {
        box_id: id,
        name: row.name,
        version: row.version,
        size: row.size,
    }
}

/// The thread of Box `t1`.
fn thread() -> SlackThread {
    SlackThread {
        channel: "C1".into(),
        thread_ts: "10.5".into(),
        box_id: BoxId::new(1),
        box_name: BoxName::parse_allow_bare("t1").unwrap(),
        owner: "U1".into(),
        status_ts: None,
    }
}

/// Shows `v` through `fake` with the test token.
fn show_via(fake: &Fake, store: &Store, v: &Published) -> Result<(), ApiError> {
    let bot = Secret::new(TOKEN.as_bytes().to_vec());
    show(store, &fake.api(), &bot, &thread(), v)
}

#[test]
fn shows_the_stored_bytes_as_a_file_in_the_thread() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let (st, id) = store_with_box(dir.path());
    let first = publish(&st, dir.path(), id, "report.txt", b"one");
    let second = publish(&st, dir.path(), id, "report.txt", b"two");
    show_via(&f, &st, &first).unwrap();
    show_via(&f, &st, &second).unwrap();
    assert_eq!(f.bodies("/upload/"), [b"one".to_vec(), b"two".to_vec()]);
    let done: Value = serde_json::from_slice(&f.bodies("completeUploadExternal")[0]).unwrap();
    assert_eq!(done["channel_id"], "C1");
    assert_eq!(done["thread_ts"], "10.5");
    assert_eq!(done["files"][0]["title"], "report.txt");
    assert!(f.texts().is_empty());
}

#[test]
fn missing_scope_posts_one_name_and_size_line() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.scope_missing.store(true, Ordering::SeqCst);
    let (st, id) = store_with_box(dir.path());
    let v = publish(&st, dir.path(), id, "report.txt", &[b'x'; 2048]);
    show_via(&f, &st, &v).unwrap();
    assert_eq!(f.texts(), ["report.txt (2.0 KB)"]);
    assert_eq!(
        f.paths(),
        ["/api/files.getUploadURLExternal", "/api/chat.postMessage"]
    );
}

#[test]
fn failed_upload_posts_the_reason_without_the_token_and_leaves_the_box() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.upload_down.store(true, Ordering::SeqCst);
    let (st, id) = store_with_box(dir.path());
    let v = publish(&st, dir.path(), id, "report.txt", b"hello");
    let name = BoxName::parse_allow_bare("t1").unwrap();
    let before = (
        st.latest_box_by_name(&name).unwrap(),
        st.events(id, 100).unwrap(),
    );
    show_via(&f, &st, &v).unwrap();
    let texts = f.texts();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].starts_with("report.txt could not be uploaded: ") && texts[0].contains("500"),
        "{texts:?}"
    );
    assert!(!texts[0].contains(TOKEN), "{texts:?}");
    assert!(f.bodies("completeUploadExternal").is_empty());
    let after = (
        st.latest_box_by_name(&name).unwrap(),
        st.events(id, 100).unwrap(),
    );
    assert_eq!(before, after, "the failed upload changed the Box");
}

#[test]
fn missing_stored_copy_posts_the_reason() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let (st, id) = store_with_box(dir.path());
    let v = Published {
        box_id: id,
        name: "gone.txt".into(),
        version: 1,
        size: 5,
    };
    show_via(&f, &st, &v).unwrap();
    assert_eq!(
        f.texts(),
        ["gone.txt could not be uploaded: no stored copy"]
    );
    assert!(f.bodies("/upload/").is_empty());
}

#[test]
fn unposted_text_is_an_error_so_the_caller_retries() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.scope_missing.store(true, Ordering::SeqCst);
    f.post_down.store(true, Ordering::SeqCst);
    let (st, id) = store_with_box(dir.path());
    let v = publish(&st, dir.path(), id, "report.txt", b"hello");
    assert!(show_via(&f, &st, &v).is_err());
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

/// Binds thread `thread_ts` of `C1` to the open Box named `name`.
fn bind(store: &Store, thread_ts: &str, name: &str) {
    let name = BoxName::parse_allow_bare(name).unwrap();
    store
        .bind_slack_thread("C1", thread_ts, &name, "U1")
        .unwrap();
}

/// Runs one follower poll of `feed` against `fake`.
fn poll(fake: &Fake, store: &Store, feed: &mut Feed) {
    let bot = Secret::new(TOKEN.as_bytes().to_vec());
    let clock = FakeClock::new(Ts::from_millis(2_000_000));
    let log = |m: &str| assert!(!m.contains(TOKEN), "token leaked: {m}");
    feed.poll(store, &fake.api(), &bot, &clock, &log);
}

/// Texts the fake saw that name `file`, failed attempts included.
fn texts_naming(fake: &Fake, file: &str) -> Vec<String> {
    let texts = fake.texts().into_iter();
    texts.filter(|t| t.contains(file)).collect()
}

#[test]
fn each_version_of_a_bound_box_is_posted_once_to_its_thread() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let (st, t1) = store_with_box(dir.path());
    let t2 = add_box(&st, "t2");
    bind(&st, "10.5", "t1");
    publish(&st, dir.path(), t1, "report.txt", b"one");
    publish(&st, dir.path(), t1, "report.txt", b"two");
    publish(&st, dir.path(), t2, "other.txt", b"unbound");
    let mut feed = Feed::default();
    poll(&f, &st, &mut feed);
    poll(&f, &st, &mut feed);
    assert_eq!(f.bodies("/upload/"), [b"one".to_vec(), b"two".to_vec()]);
    bind(&st, "20.5", "t2");
    poll(&f, &st, &mut feed);
    poll(&f, &st, &mut feed);
    let uploads = f.bodies("/upload/");
    assert_eq!(
        uploads,
        [b"one".to_vec(), b"two".to_vec(), b"unbound".to_vec()]
    );
    let done = f.bodies("completeUploadExternal");
    let thread_of = |b: &Vec<u8>| serde_json::from_slice::<Value>(b).unwrap()["thread_ts"].clone();
    let threads: Vec<Value> = done.iter().map(thread_of).collect();
    assert_eq!(threads, [json!("10.5"), json!("10.5"), json!("20.5")]);
}

#[test]
fn restart_replays_the_feed_without_posting_a_version_again() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let (st, id) = store_with_box(dir.path());
    bind(&st, "10.5", "t1");
    publish(&st, dir.path(), id, "report.txt", b"one");
    publish(&st, dir.path(), id, "shot.png", b"png");
    poll(&f, &st, &mut Feed::default());
    drop(st);
    let st = open_store(dir.path());
    let mut feed = Feed::default();
    poll(&f, &st, &mut feed);
    poll(&f, &st, &mut feed);
    assert_eq!(f.bodies("/upload/"), [b"one".to_vec(), b"png".to_vec()]);
    publish(&st, dir.path(), id, "report.txt", b"two");
    poll(&f, &st, &mut feed);
    let uploads = f.bodies("/upload/");
    assert_eq!(uploads, [b"one".to_vec(), b"png".to_vec(), b"two".to_vec()]);
    assert_eq!(f.bodies("completeUploadExternal").len(), 3);
}

#[test]
fn a_reused_box_name_does_not_get_another_box_artifacts() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    let (st, old) = store_with_box(dir.path());
    bind(&st, "10.5", "t1");
    let close = Decision::new(old).lifecycle(BoxLifecycleInput::Close);
    st.apply(close).unwrap();
    let new = add_box(&st, "t1");
    publish(&st, dir.path(), new, "report.txt", b"new box");
    poll(&f, &st, &mut Feed::default());
    assert!(f.bodies("/upload/").is_empty());
    assert!(texts_naming(&f, "report.txt").is_empty());
}

#[test]
fn a_version_nothing_reached_the_thread_for_is_retried() {
    let (dir, f) = (tempfile::tempdir().unwrap(), fake());
    f.scope_missing.store(true, Ordering::SeqCst);
    f.post_down.store(true, Ordering::SeqCst);
    let (st, id) = store_with_box(dir.path());
    bind(&st, "10.5", "t1");
    publish(&st, dir.path(), id, "report.txt", b"hello");
    let mut feed = Feed::default();
    poll(&f, &st, &mut feed);
    assert!(!st.slack_artifact_posted(id, "report.txt", 1).unwrap());
    f.post_down.store(false, Ordering::SeqCst);
    poll(&f, &st, &mut feed);
    poll(&f, &st, &mut feed);
    assert_eq!(texts_naming(&f, "report.txt"), ["report.txt (5 B)"; 2]);
    assert!(st.slack_artifact_posted(id, "report.txt", 1).unwrap());
}
