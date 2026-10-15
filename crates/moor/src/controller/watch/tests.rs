#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "tests wait on real threads with deadlines"
)]
use std::io::{self, Cursor, Write};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{run, stream, Limits, Shared};
use crate::controller::conn::handle;
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use moor_core::domain::decision::{Decision, ExtraEvent, NewTask};
use moor_core::seam::clock::Ts;
use moor_core::{TaskId, TaskName};

fn env(dir: &tempfile::TempDir) -> Env {
    Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().expect("utf8 path")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ])
}

fn fast(buffer: usize) -> Limits {
    Limits {
        buffer,
        poll: Duration::from_millis(10),
        batch: 3,
    }
}

fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join(STORE_FILE), clock).expect("store")
}

fn add(store: &Store, name: &str) -> TaskId {
    let new = NewTask {
        name: TaskName::parse(name).expect("name"),
        repo: "r".into(),
    };
    store.apply(Decision::insert(new)).expect("insert").task
}

/// Append `n` `copied` events to `id`.
fn append(store: &Store, id: TaskId, n: usize) {
    for i in 0..n {
        let event = ExtraEvent::Copied {
            from: format!("a{i}"),
            to: format!("b{i}"),
        };
        store.apply(Decision::new(id).event(event)).expect("event");
    }
}

/// Every stored event id, ascending.
fn ids(store: &Store) -> Vec<i64> {
    let all = store.events_after(0, 10_000).expect("events");
    all.iter().map(|e| e.row.id).collect()
}

/// A writer that records frames and can stall or fail on purpose.
struct Probe {
    out: Arc<Mutex<Vec<u8>>>,
    /// Flush fails once this many frames were written.
    limit: usize,
    /// The first write waits for a message here.
    gate: Option<Receiver<()>>,
    frames: usize,
}

impl Probe {
    fn new(limit: usize) -> (Self, Arc<Mutex<Vec<u8>>>) {
        let out = Arc::new(Mutex::new(Vec::new()));
        let probe = Self {
            out: Arc::clone(&out),
            limit,
            gate: None,
            frames: 0,
        };
        (probe, out)
    }
}

impl Write for Probe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            gate.recv().expect("released");
        }
        self.out.lock().expect("lock").extend_from_slice(buf);
        self.frames += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.frames >= self.limit {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
}

fn frames(out: &Mutex<Vec<u8>>) -> Vec<Value> {
    let bytes = out.lock().expect("lock").clone();
    String::from_utf8(bytes)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json frame"))
        .collect()
}

fn cursors(frames: &[Value]) -> Vec<i64> {
    frames
        .iter()
        .map(|f| f["cursor"].as_i64().expect("cursor"))
        .collect()
}

#[test]
fn stream_from_zero_delivers_every_event_in_order_with_task_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 2);
    let stored = store.events_after(0, 100).expect("events");
    let (mut probe, out) = Probe::new(stored.len());
    stream(&mut probe, &env(&dir), 0, fast(16));
    let got = frames(&out);
    assert_eq!(got.len(), stored.len());
    for (frame, event) in got.iter().zip(&stored) {
        assert_eq!(frame["frame"], "event");
        assert_eq!(frame["cursor"], event.row.id);
        assert_eq!(frame["task"], "alpha");
        assert_eq!(frame["kind"], event.row.kind.as_str());
        assert_eq!(frame["payload"], event.row.payload);
        assert_eq!(frame["ts"], event.row.ts.as_millis());
    }
    assert_eq!(got.last().expect("last")["payload"]["to"], "b1");
}

#[test]
fn stream_follows_events_appended_after_it_started() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    let seen = ids(&store).len();
    let (mut probe, out) = Probe::new(seen + 2);
    let env = env(&dir);
    let watcher = thread::spawn(move || stream(&mut probe, &env, 0, fast(16)));
    thread::sleep(Duration::from_millis(50));
    append(&store, id, 2);
    watcher.join().expect("watcher");
    assert_eq!(cursors(&frames(&out)), ids(&store));
}

#[test]
fn reconnect_from_cursor_has_no_duplicates_or_gaps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    let have = ids(&store).len();
    append(&store, id, 5usize.saturating_sub(have).max(1));
    let (mut first, out1) = Probe::new(3);
    stream(&mut first, &env(&dir), 0, fast(16));
    let run1 = cursors(&frames(&out1));
    assert_eq!(run1.len(), 3);

    append(&store, id, 2);
    let all = ids(&store);
    let resume = *run1.last().expect("third cursor");
    let (mut second, out2) = Probe::new(all.len() - 3);
    stream(&mut second, &env(&dir), resume, fast(16));
    let run2 = cursors(&frames(&out2));

    assert!(run2.windows(2).all(|w| w[0] < w[1]), "{run2:?}");
    assert_eq!(run2, all[3..], "exactly the events after the cursor");
    let union: Vec<i64> = run1.iter().chain(&run2).copied().collect();
    assert_eq!(union, all, "no duplicate and no gap across both runs");
}

#[test]
fn slow_client_is_dropped_without_blocking_the_poller() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 10);
    let all = ids(&store);
    let (mut probe, out) = Probe::new(usize::MAX);
    let (release, gate) = channel();
    probe.gate = Some(gate);
    let shared = Arc::new(Shared::default());
    let flags = Arc::clone(&shared);
    let env = env(&dir);
    let watcher = thread::spawn(move || run(&mut probe, &env, 0, fast(4), &flags));

    let deadline = Instant::now() + Duration::from_secs(4);
    while !shared.overflow.load(std::sync::atomic::Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "poller never gave up on a stalled client"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(frames(&out).is_empty(), "client was still blocked");
    release.send(()).expect("release");
    watcher.join().expect("watcher");

    let got = frames(&out);
    let (error, events) = got.split_last().expect("frames");
    assert_eq!(error["frame"], "error");
    assert_eq!(error["kind"], "slow-client");
    let delivered = cursors(events);
    assert!(!delivered.is_empty() && delivered.len() < all.len());
    assert_eq!(delivered, all[..delivered.len()], "gap-free prefix");
    let last = delivered.last().expect("last");
    assert!(error["message"]
        .as_str()
        .expect("message")
        .contains(&last.to_string()));
}

#[test]
fn watch_before_hello_is_refused_and_the_connection_keeps_serving() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = [
        json!({"frame": "watch", "since": 0}),
        json!({"frame": "hello", "protocol": 1}),
        json!({"frame": "request", "id": "r1", "request": {"cmd": "version"}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let mut out = Vec::new();
    handle(Cursor::new(input.into_bytes()), &mut out, &env(&dir));
    let got: Vec<Value> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json frame"))
        .collect();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0]["kind"], "hello-required");
    assert_eq!(got[1]["frame"], "hello");
    assert_eq!(got[2]["id"], "r1");
}

#[test]
fn unopenable_store_ends_the_stream_with_a_store_unavailable_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("plain-file");
    std::fs::write(&file, b"x").expect("file");
    let under_file = file.join("state");
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", under_file.to_str().expect("utf8 path")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let (mut probe, out) = Probe::new(usize::MAX);
    stream(&mut probe, &env, 7, fast(4));

    let got = frames(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["frame"], "error");
    assert_eq!(got[0]["kind"], "store-unavailable");
    assert!(got[0]["message"].as_str().expect("message").contains('7'));
}
