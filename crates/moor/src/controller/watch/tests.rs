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

use moor_core::user::{UserId, Viewer};
use serde_json::{json, Value};

use super::{run, stream, stream_draining, Ask, Limits, Shared};
use crate::controller::conn::handle;
use crate::env::Env;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use moor_core::controller::WatchFilter;
use moor_core::domain::decision::{Decision, ExtraEvent, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, BoxName, SessionId};

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

fn ask(since: i64, filter: WatchFilter, follow: bool) -> Ask {
    Ask {
        since,
        filter,
        follow,
        viewer: Viewer::Operator,
    }
}

fn everything() -> WatchFilter {
    WatchFilter::default()
}

fn open(dir: &Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    Store::open(&dir.join(STORE_FILE), clock).expect("store")
}

fn add(store: &Store, name: &str) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).expect("name"),
        repo: None,
    };
    store.apply(Decision::insert(new)).expect("insert").box_id
}

/// Append `n` `copied` events to `id`.
fn append(store: &Store, id: BoxId, n: usize) {
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
fn stream_from_zero_delivers_every_event_in_order_with_box_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 2);
    let stored = store.events_after(0, 100).expect("events");
    let (mut probe, out) = Probe::new(stored.len());
    stream(&mut probe, &env(&dir), ask(0, everything(), true), fast(16));
    let got = frames(&out);
    assert_eq!(got.len(), stored.len());
    for (frame, event) in got.iter().zip(&stored) {
        assert_eq!(frame["frame"], "event");
        assert_eq!(frame["cursor"], event.row.id);
        assert_eq!(frame["box"], "alpha");
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
    let watcher =
        thread::spawn(move || stream(&mut probe, &env, ask(0, everything(), true), fast(16)));
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
    stream(&mut first, &env(&dir), ask(0, everything(), true), fast(16));
    let run1 = cursors(&frames(&out1));
    assert_eq!(run1.len(), 3);

    append(&store, id, 2);
    let all = ids(&store);
    let resume = *run1.last().expect("third cursor");
    let (mut second, out2) = Probe::new(all.len() - 3);
    stream(
        &mut second,
        &env(&dir),
        ask(resume, everything(), true),
        fast(16),
    );
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
    let watcher = thread::spawn(move || {
        run(
            &mut probe,
            &env,
            ask(0, everything(), true),
            fast(4),
            &flags,
        );
    });

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
        json!({"frame": "hello", "protocol": moor_core::controller::PROTOCOL}),
        json!({"frame": "request", "id": "r1", "request": {"cmd": "version"}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let mut out = Vec::new();
    let peer = crate::controller::conn::Peer::local_in(dir.path());
    handle(Cursor::new(input.into_bytes()), &mut out, &env(&dir), &peer);
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
    stream(&mut probe, &env, ask(7, everything(), true), fast(4));

    let got = frames(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["frame"], "error");
    assert_eq!(got[0]["kind"], "store-unavailable");
    assert!(got[0]["message"].as_str().expect("message").contains('7'));
}

fn only_boxes(names: &[&str]) -> WatchFilter {
    WatchFilter {
        boxes: Some(
            names
                .iter()
                .map(|n| BoxName::parse_allow_bare(n).expect("name"))
                .collect(),
        ),
        kinds: None,
        parent: None,
    }
}

fn only_kinds(kinds: &[&str]) -> WatchFilter {
    WatchFilter {
        boxes: None,
        kinds: Some(kinds.iter().map(|k| (*k).to_string()).collect()),
        parent: None,
    }
}

fn read_once(dir: &tempfile::TempDir, since: i64, filter: &WatchFilter) -> Vec<Value> {
    let (mut probe, out) = Probe::new(usize::MAX);
    stream(
        &mut probe,
        &env(dir),
        ask(since, filter.clone(), false),
        fast(16),
    );
    frames(&out)
}

#[test]
fn kinds_filter_delivers_only_those_kinds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 3);
    let got = read_once(&dir, 0, &only_kinds(&["copied"]));
    let (_, got) = got.split_last().expect("caught-up frame");
    assert_eq!(got.len(), 3);
    assert!(got.iter().all(|f| f["kind"] == "copied"), "{got:?}");
    assert!(ids(&store).len() > 3, "other kinds exist and were skipped");
}

#[test]
fn boxes_filter_delivers_only_those_boxes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let alpha = add(&store, "alpha");
    let beta = add(&store, "beta");
    append(&store, alpha, 2);
    append(&store, beta, 2);
    let got = read_once(&dir, 0, &only_boxes(&["beta"]));
    let (_, got) = got.split_last().expect("caught-up frame");
    assert!(!got.is_empty());
    assert!(got.iter().all(|f| f["box"] == "beta"), "{got:?}");
    assert!(got.len() < ids(&store).len());
}

#[test]
fn skipped_events_advance_the_cursor_so_resume_has_no_gap_or_duplicates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let alpha = add(&store, "alpha");
    let beta = add(&store, "beta");
    append(&store, alpha, 4);
    append(&store, beta, 4);
    let filter = only_boxes(&["beta"]);
    let events = |since| {
        let got = read_once(&dir, since, &filter);
        cursors(&got[..got.len() - 1])
    };
    let full = events(0);
    let resume = full[1];
    let rest = events(resume);
    assert_eq!(rest, full[2..], "resume yields exactly the remainder");
    let last = *ids(&store).last().expect("last");
    assert!(events(last).is_empty());
}

#[test]
fn without_follow_the_stream_ends_with_one_caught_up_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 7);
    let got = read_once(&dir, 0, &everything());
    let (last, events) = got.split_last().expect("frames");
    assert_eq!(cursors(events), ids(&store));
    assert!(events.iter().all(|f| f["frame"] == "event"), "{got:?}");
    assert_eq!(last["frame"], "caught-up");
    assert_eq!(last["cursor"], *ids(&store).last().expect("newest"));
}

#[test]
fn caught_up_cursor_counts_trailing_events_the_filter_skipped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let alpha = add(&store, "alpha");
    let beta = add(&store, "beta");
    append(&store, beta, 2);
    append(&store, alpha, 4);
    let filter = only_boxes(&["beta"]);
    let got = read_once(&dir, 0, &filter);
    let (last, events) = got.split_last().expect("frames");
    assert!(!events.is_empty());
    assert!(events.iter().all(|f| f["frame"] == "event"), "{got:?}");
    let newest = *ids(&store).last().expect("newest");
    assert_eq!(last["frame"], "caught-up");
    assert_eq!(last["cursor"], newest);
    assert!(newest > *cursors(events).last().expect("delivered"));

    let resumed = read_once(&dir, newest, &filter);
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert_eq!(resumed[0]["frame"], "caught-up");
    assert_eq!(resumed[0]["cursor"], newest);
}

#[test]
fn empty_store_catches_up_at_the_requested_cursor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let got = read_once(&dir, 0, &everything());
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["frame"], "caught-up");
    assert_eq!(got[0]["cursor"], 0);
    let nothing = read_once(&dir, 0, &only_kinds(&["nothing"]));
    assert_eq!(nothing.len(), 1);
    assert_eq!(nothing[0]["frame"], "caught-up");
}

#[test]
fn a_following_watch_ends_with_restarting_once_the_drain_begins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let id = add(&store, "alpha");
    append(&store, id, 1);
    let last = *ids(&store).last().expect("an event");
    let drain = Arc::new(crate::controller::drain::Drain::default());
    let (mut probe, out) = Probe::new(usize::MAX);
    let env = env(&dir);
    let streaming = Arc::clone(&drain);
    let (done, ended) = channel();
    thread::spawn(move || {
        stream_draining(
            &mut probe,
            &env,
            ask(0, everything(), true),
            fast(16),
            &streaming,
        );
        done.send(()).ok();
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        ended.try_recv().is_err(),
        "the watch ended before the drain"
    );
    drain.begin();
    ended
        .recv_timeout(Duration::from_secs(5))
        .expect("the watch kept following after the drain began");
    let got = frames(&out);
    let tail = got.last().expect("frames");
    assert_eq!(tail["frame"], "error", "{got:?}");
    assert_eq!(tail["kind"], "restarting", "{got:?}");
    assert!(tail["message"]
        .as_str()
        .expect("message")
        .contains(&format!("cursor {last}")));
}

/// A Box `name` whose primary Session is a child of Session `parent`.
fn add_child(store: &Store, name: &str, parent: i64) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).expect("name"),
        repo: None,
    };
    let decision = Decision::insert(new).parent_session(Some(parent));
    store.apply(decision).expect("insert child").box_id
}

fn only_parent(parent: i64) -> WatchFilter {
    WatchFilter {
        parent: Some(SessionId::new(parent)),
        ..WatchFilter::default()
    }
}

/// The Boxes named by the event frames in `got`, without the caught-up frame.
fn event_boxes(got: &[Value]) -> Vec<String> {
    got.iter()
        .filter(|f| f["frame"] == "event")
        .map(|f| f["box"].as_str().expect("box").to_owned())
        .collect()
}

/// Two parents with one child Box each, and an unrelated Box with no parent.
fn family(dir: &Path) -> (i64, i64) {
    let store = open(dir);
    let one = add(&store, "one");
    let two = add(&store, "two");
    let session = |id: BoxId| {
        store
            .events_after(0, 10_000)
            .expect("events")
            .iter()
            .find(|e| e.row.box_id == id && e.session.is_some())
            .and_then(|e| e.session)
            .expect("a session event")
            .get()
    };
    let (p1, p2) = (session(one), session(two));
    let c1 = add_child(&store, "kid-one", p1);
    let c2 = add_child(&store, "kid-two", p2);
    append(&store, c1, 1);
    append(&store, c2, 1);
    (p1, p2)
}

#[test]
fn parent_filter_delivers_only_the_children_of_that_parent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (p1, _) = family(dir.path());
    let got = read_once(&dir, 0, &only_parent(p1));
    let boxes = event_boxes(&got);
    assert!(!boxes.is_empty(), "{got:?}");
    assert!(boxes.iter().all(|b| b == "kid-one"), "{boxes:?}");
}

#[test]
fn parent_filter_with_no_children_delivers_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    family(dir.path());
    let got = read_once(&dir, 0, &only_parent(9_999));
    assert!(event_boxes(&got).is_empty(), "{got:?}");
    assert_eq!(got.last().expect("frame")["frame"], "caught-up");
}

#[test]
fn parent_filter_keeps_the_viewers_own_boxes_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let raw = rusqlite::Connection::open(dir.path().join(STORE_FILE)).expect("raw");
    raw.execute(
        "INSERT INTO users (id, created_at) VALUES (1, 0), (2, 0)",
        [],
    )
    .expect("users");
    add(&store, "root");
    let parent = store
        .events_after(0, 10)
        .expect("events")
        .iter()
        .find_map(|e| e.session)
        .expect("session")
        .get();
    for (name, user) in [("mine", 1), ("theirs", 2)] {
        let new = NewBox {
            name: BoxName::parse_allow_bare(name).expect("name"),
            repo: None,
        };
        let decision = Decision::insert(new)
            .parent_session(Some(parent))
            .requester(UserId::new(user));
        let id = store.apply(decision).expect("insert").box_id;
        append(&store, id, 1);
    }
    let (mut probe, out) = Probe::new(usize::MAX);
    let viewer = Viewer::User(UserId::new(1));
    let mut asked = ask(0, only_parent(parent), false);
    asked.viewer = viewer;
    stream(&mut probe, &env(&dir), asked, fast(64));
    let boxes = event_boxes(&frames(&out));
    assert!(!boxes.is_empty());
    assert!(boxes.iter().all(|b| b == "mine"), "{boxes:?}");
}

#[test]
fn an_event_with_no_session_never_matches_a_parent_filter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let moor = crate::Moor::from_env(&env(&dir)).expect("moor");
    let asked = ask(0, only_parent(1), false);
    let mut passes = super::Passes {
        moor: &moor,
        ask: &asked,
        seen: std::collections::HashMap::new(),
        parents: std::collections::HashMap::new(),
    };
    let frame = moor_core::controller::ServerFrame::Event {
        cursor: 1,
        r#box: None,
        box_id: None,
        session: None,
        kind: "copied".to_owned(),
        payload: json!({}),
        ts: 1,
    };
    assert_eq!(passes.check(&frame), Ok(false));
}
