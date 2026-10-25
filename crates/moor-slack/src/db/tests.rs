//! Tests for `slack.db`: event dedupe, the schema version, and threads with their Boxes.

use std::path::Path;

use moor_core::name::{BoxId, BoxName};

use super::BASELINE_VERSION;
use super::{BoxState, Db, InboxItem, ItemKind, SlackBox, SlackThread};

fn open(dir: &Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

fn starting(thread_ts: &str, name: &str, owner: &str) -> SlackBox {
    SlackBox {
        channel: "C1".into(),
        thread_ts: thread_ts.into(),
        box_name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "api".into(),
        goal: "fix it".into(),
        base: None,
        requested_by: owner.into(),
        parent: None,
        state: BoxState::Starting,
    }
}

/// Records the thread of `boxed`, started by its requester, then `boxed` itself.
fn open_with(db: &Db, boxed: &SlackBox, now: i64) -> rusqlite::Result<()> {
    let thread = SlackThread {
        channel: boxed.channel.clone(),
        thread_ts: boxed.thread_ts.clone(),
        owner: boxed.requested_by.clone(),
    };
    db.atomically(|| {
        db.insert_thread(&thread, now)?;
        db.insert_starting(boxed, now)
    })
}

/// The Box of thread `thread_ts` in channel `C1`, if it has one.
fn box_of(db: &Db, thread_ts: &str) -> Option<SlackBox> {
    db.thread_boxes("C1", thread_ts).unwrap().pop()
}

#[test]
fn event_is_new_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    assert!(db.event_once("Ev1", 1).unwrap());
    assert!(!db.event_once("Ev1", 2).unwrap());
    assert!(db.event_once("Ev2", 3).unwrap());
    drop(db);
    assert!(!open(dir.path()).event_once("Ev1", 4).unwrap());
}

#[test]
fn thread_binding_first_wins() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    assert_eq!(box_of(&db, "1.1"), None);
    let first = starting("1.1", "first", "U1");
    open_with(&db, &first, 1).unwrap();
    assert!(open_with(&db, &starting("1.1", "second", "U2"), 2).is_err());
    assert_eq!(db.thread_boxes("C1", "1.1").unwrap(), [first]);
    assert_eq!(db.thread("C1", "1.1").unwrap().unwrap().owner, "U1");
    assert_eq!(box_of(&db, "2.2"), None);
}

#[test]
fn owner_kept_from_first_bind() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let first = starting("1.1", "first", "U1");
    open_with(&db, &first, 1).unwrap();
    db.bind(&first.box_name, BoxId::new(7), Some("moor/first"))
        .unwrap();
    db.bind(&first.box_name, BoxId::new(8), None).unwrap();
    let row = box_of(&db, "1.1").unwrap();
    assert_eq!(row.requested_by, "U1");
    let want = BoxState::Bound {
        box_id: BoxId::new(7),
        branch: Some("moor/first".into()),
    };
    assert_eq!(row.state, want);
}

#[test]
fn starting_rows_are_listed_oldest_first_and_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let (b, a, c) = (
        starting("2.2", "b", "U1"),
        starting("1.1", "a", "U1"),
        starting("3.3", "c", "U1"),
    );
    open_with(&db, &b, 5).unwrap();
    open_with(&db, &a, 9).unwrap();
    open_with(&db, &c, 1).unwrap();
    db.bind(&c.box_name, BoxId::new(3), None).unwrap();
    db.forget_starting(&c.box_name).unwrap();
    let names: Vec<String> = db
        .starting()
        .unwrap()
        .iter()
        .map(|t| t.box_name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["b", "a"]);
    db.forget_starting(&b.box_name).unwrap();
    assert_eq!(box_of(&db, "2.2"), None);
    assert!(db.thread("C1", "2.2").unwrap().is_some());
    drop(db);
    let db = open(dir.path());
    assert_eq!(db.starting().unwrap().len(), 1);
    assert!(box_of(&db, "3.3").is_some());
}

/// Writes a `slack.db` at `version` with one table, as an older or newer build would leave it.
fn write_at_version(path: &Path, version: i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE old_rows (id INTEGER);")
        .unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
}

/// Every file in `dir` with its bytes.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).unwrap())
        })
        .collect();
    files.sort();
    files
}

#[test]
fn newer_schema_is_refused() {
    for found in [1, BASELINE_VERSION - 1, BASELINE_VERSION + 1] {
        let dir = tempfile::tempdir().unwrap();
        write_at_version(&dir.path().join("slack.db"), found);
        let err = Db::open(&dir.path().join("slack.db")).err().unwrap();
        let want = format!(
            "slack.db is schema {found} and this build needs {BASELINE_VERSION}. Start with a fresh slack.db."
        );
        assert_eq!(err, want);
    }
}

#[test]
fn slack_db_that_keeps_the_box_in_the_thread_row_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE slack_threads (channel TEXT, thread_ts TEXT, state TEXT, box_id INTEGER, \
         box_name TEXT, repo TEXT, goal TEXT, owner TEXT); PRAGMA user_version = 3;",
    )
    .unwrap();
    drop(conn);
    let err = Db::open(&path).err().unwrap();
    assert!(err.starts_with("slack.db is schema 3 "), "{err}");
}

#[test]
fn refused_file_is_left_as_found() {
    for version in [0, 1, 99] {
        let dir = tempfile::tempdir().unwrap();
        write_at_version(&dir.path().join("slack.db"), version);
        let before = snapshot(dir.path());
        assert!(Db::open(&dir.path().join("slack.db")).is_err());
        assert_eq!(snapshot(dir.path()), before, "version {version}");
    }
}

#[test]
fn refused_file_with_an_uncheckpointed_wal_is_left_as_found() {
    let writer_dir = tempfile::tempdir().unwrap();
    let writer = rusqlite::Connection::open(writer_dir.path().join("slack.db")).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    writer
        .execute_batch("CREATE TABLE old_rows (id INTEGER); PRAGMA user_version = 99;")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    for name in ["slack.db", "slack.db-wal"] {
        std::fs::copy(writer_dir.path().join(name), dir.path().join(name)).unwrap();
    }
    drop(writer);
    let read = |name: &str| std::fs::read(dir.path().join(name)).ok();
    let (db, wal) = (read("slack.db"), read("slack.db-wal"));
    assert!(wal.as_ref().is_some_and(|w| !w.is_empty()));
    let err = Db::open(&dir.path().join("slack.db")).err().unwrap();
    assert!(err.starts_with("slack.db is schema 99 "), "{err}");
    assert!(read("slack.db") == db, "slack.db changed");
    assert!(read("slack.db-wal") == wal, "slack.db-wal changed");
}

#[test]
fn threads_opening_a_new_file_at_once_create_the_schema_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    // Holding the write lock lets every thread read an empty file before any can write.
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let opens: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || Db::open(&path).map(drop))
        })
        .collect();
    std::thread::sleep(std::time::Duration::from_millis(500));
    blocker.execute_batch("COMMIT;").unwrap();
    for open in opens {
        assert_eq!(open.join().unwrap(), Ok(()));
    }
}

#[test]
fn new_file_gets_the_schema_once_and_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let first = Db::open(&path).unwrap();
    let second = Db::open(&path).unwrap();
    assert!(first.event_once("Ev1", 1).unwrap());
    assert!(!second.event_once("Ev1", 2).unwrap());
}

#[test]
fn slack_db_without_an_inbox_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE slack_boxes (box_name TEXT PRIMARY KEY); PRAGMA user_version = 4;",
    )
    .unwrap();
    drop(conn);
    let err = Db::open(&path).err().unwrap();
    assert!(err.starts_with("slack.db is schema 4 "), "{err}");
}

/// Opens thread `thread_ts` at time 0 with host Session `session` recorded.
fn live_thread(db: &Db, thread_ts: &str, session: i64) {
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: thread_ts.into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 0).unwrap();
    db.set_root_session("C1", thread_ts, "/w", session).unwrap();
}

/// Queues a message from `U1` in thread `thread_ts`.
fn message(db: &Db, thread_ts: &str, source: &str) {
    let item = InboxItem {
        channel: "C1".into(),
        thread_ts: thread_ts.into(),
        kind: ItemKind::Message,
        author: "U1".into(),
        text: "hi".into(),
        message_ts: None,
        source: source.into(),
    };
    db.queue(&item, 0).unwrap();
}

#[test]
fn a_thread_is_quiet_only_with_no_turn_waiting_message_or_starting_box_since_the_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let threads = [
        ("1.0", 100),
        ("2.0", 101),
        ("3.0", 102),
        ("4.0", 103),
        ("5.0", 104),
    ];
    for (ts, session) in threads {
        live_thread(&db, ts, session);
    }
    message(&db, "2.0", "e2");
    message(&db, "3.0", "e3");
    let turn = db.take_turn(5).unwrap().unwrap();
    assert_eq!(turn.thread_ts, "2.0");
    db.store_reply(turn.id, super::End::Replied, "OK", None, None)
        .unwrap();
    db.mark_posted(turn.id, 50).unwrap();
    let open = db.take_turn(60).unwrap().unwrap();
    assert_eq!(open.thread_ts, "3.0");
    db.insert_starting(&starting("4.0", "slack-4-1", "U1"), 0)
        .unwrap();
    message(&db, "5.0", "e5");
    let quiet = |cutoff| db.quiet_threads(cutoff).unwrap();
    assert_eq!(quiet(40), [("C1".to_owned(), "1.0".to_owned(), 100)]);
    let after = [
        ("C1".to_owned(), "1.0".to_owned(), 100),
        ("C1".to_owned(), "2.0".to_owned(), 101),
    ];
    assert_eq!(quiet(50), after, "a turn that ended at the cutoff is quiet");
    db.clear_root_session("C1", "1.0", 99).unwrap();
    assert_eq!(
        quiet(50).len(),
        2,
        "another Session id leaves the record alone"
    );
    db.clear_root_session("C1", "1.0", 100).unwrap();
    assert_eq!(quiet(50), [("C1".to_owned(), "2.0".to_owned(), 101)]);
}

#[test]
fn a_file_from_the_last_build_before_migrations_is_adopted_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slack.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../../migrations/V1__baseline.sql"))
        .unwrap();
    conn.pragma_update(None, "user_version", BASELINE_VERSION)
        .unwrap();
    conn.execute(
        "INSERT INTO slack_events (event_id, seen_at) VALUES ('Ev1', 1)",
        [],
    )
    .unwrap();
    drop(conn);
    let db = Db::open(&path).unwrap();
    assert!(!db.event_once("Ev1", 2).unwrap(), "the old row is kept");
    let applied: i64 = db
        .conn
        .query_row("SELECT count(*) FROM refinery_schema_history", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(applied, 1);
}

#[test]
fn applied_migrations_are_never_edited() {
    let pinned: Vec<(i32, u64)> = super::schema()
        .unwrap()
        .iter()
        .map(|m| (m.version(), m.checksum()))
        .collect();
    assert_eq!(
        pinned,
        [(1, 8_754_816_789_016_865_323)],
        "a schema change is a new migration file; an applied one must not change"
    );
}
