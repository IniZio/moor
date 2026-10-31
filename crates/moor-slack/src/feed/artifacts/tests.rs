//! Artifact versions in a Box's thread, against a fake Slack and a fake controller socket.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index built values"
)]

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use moor_client::{Client, EventRecord, WatchItem};
use moor_core::secret::Secret;
use serde_json::{json, Value};

use super::super::status::Planner;
use super::super::tests::bound_box;
use super::super::{now, Fold, Folds};
use super::*;
use crate::api::Api;
use crate::dial::Watched;
use crate::identity::{Link, Team};
use crate::upload::tests::{controller, fake, sha, Fake, Stored};
use moor_core::user::Identity;

/// The id [`bound_box`] binds its Box to.
const ID: BoxId = BoxId::new(7);

thread_local! {
    /// The time the feed under test reads, per test thread.
    static NOW: std::cell::Cell<i64> = const { std::cell::Cell::new(9) };
}

/// The feed's clock.
fn clock() -> i64 {
    NOW.with(std::cell::Cell::get)
}

/// Moves the feed's clock to `t` unix seconds.
fn at(t: i64) {
    NOW.with(|n| n.set(t));
}

/// The requests the fake controller saw, failing after about five seconds when it still
/// waits for more requests than the feed sent.
fn seen(ctl: std::thread::JoinHandle<Vec<Value>>) -> Vec<Value> {
    for _ in 0..500 {
        if ctl.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        ctl.is_finished(),
        "the fake controller still waits for a read the feed never sent"
    );
    ctl.join().unwrap()
}

/// A feed that posts to `slack` and reads artifacts from the controller at `sock`.
fn feed(dir: &Path, slack: &Fake, sock: &Path) -> Feed {
    feed_logging(dir, slack, sock, Arc::new(|_| {}))
}

/// Like [`feed`], writing its log to `log`.
fn feed_logging(dir: &Path, slack: &Fake, sock: &Path, log: crate::Log) -> Feed {
    Feed {
        db_path: dir.join("slack.db"),
        api: Arc::new(Api::new(&slack.base).unwrap()),
        bot_token: Arc::new(Secret::new(b"xoxb-test".to_vec())),
        link: Link::new(Client::new(sock), Team::parse("T1").unwrap()),
        log,
        clock,
        long_turn_secs: 1200,
    }
}

/// The caught-up fold of a connection that replays the publishing of `bytes` at each version
/// in `versions`, as `report.txt` of Box [`ID`].
fn replay(versions: &[u32], bytes: &[u8]) -> Fold {
    replay_for(ID, "slack-1-1", versions, bytes)
}

/// Like [`replay`], for Box `id` named `name`.
fn replay_for(id: BoxId, name: &str, versions: &[u32], bytes: &[u8]) -> Fold {
    let mut fold = Fold::default();
    fold.take(Watched::Connected);
    for (cursor, &version) in (1..).zip(versions) {
        let payload = json!({
            "name": "report.txt", "version": version, "artifact_kind": "file",
            "content_type": "text/plain", "size": bytes.len(), "sha256": sha(bytes),
        });
        fold.take(Watched::Item(WatchItem::Event(EventRecord {
            cursor,
            box_name: Some(name.into()),
            box_id: Some(id),
            session: None,
            kind: "artifact_published".into(),
            payload,
            ts: 0,
        })));
    }
    fold.take(Watched::Item(WatchItem::CaughtUp { cursor: 0 }));
    fold
}

/// One feed tick that is due to send lines.
fn tick(feed: &Feed, db: &Db, fold: &mut Fold) {
    let owner = Identity::parse("slack:T1/U1").unwrap();
    let mut folds = Folds(std::iter::once((owner.clone(), std::mem::take(fold))).collect());
    feed.act(db, &mut folds, &mut Planner::default(), &mut now());
    *fold = folds.0.remove(&owner).unwrap();
}

/// The files Slack shared, by title.
fn shared(slack: &Fake) -> Vec<String> {
    let title = |(_, b): &(u64, Vec<u8>)| {
        let v: Value = serde_json::from_slice(b).unwrap();
        v["files"][0]["title"].as_str().unwrap().to_owned()
    };
    slack
        .hits("completeuploadexternal")
        .iter()
        .map(title)
        .collect()
}

/// The texts posted that tell of a failed upload.
fn failures(slack: &Fake) -> Vec<String> {
    let mut texts = slack.texts();
    texts.retain(|t| t.contains("could not be uploaded"));
    texts
}

/// The versions the controller was asked to read, each asked as the Box's owner.
fn reads(asked: &[Value]) -> Vec<u64> {
    asked
        .iter()
        .map(|r| {
            assert_eq!(r["cmd"], "as", "{r}");
            assert_eq!(r["identity"], "slack:T1/U1", "{r}");
            r["request"]["version"].as_u64().unwrap()
        })
        .collect()
}

#[test]
fn a_version_goes_up_once_across_a_refold_and_a_restart() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    tick(&feed, &db, &mut fold);
    tick(&feed, &db, &mut fold);
    let mut fold = replay(&[1], b"hello");
    tick(&feed, &db, &mut fold);
    drop(db);
    let db = Db::open(&feed.db_path).unwrap();
    let mut fold = replay(&[1], b"hello");
    tick(&feed, &db, &mut fold);
    assert_eq!(reads(&seen(ctl)), [1], "read once");
    assert_eq!(shared(&slack), ["report.txt"], "shared once");
    assert!(failures(&slack).is_empty(), "{:?}", slack.texts());
}

#[test]
fn a_second_version_goes_up_once_after_the_first() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello"), Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    tick(&feed, &db, &mut fold);
    let mut fold = replay(&[1, 2], b"hello");
    tick(&feed, &db, &mut fold);
    tick(&feed, &db, &mut fold);
    assert_eq!(reads(&seen(ctl)), [1, 2]);
    assert_eq!(shared(&slack), ["report.txt", "report.txt"]);
}

#[test]
fn a_failed_upload_is_told_once_and_tried_again_with_a_wait_until_it_goes_up() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.upload_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let stored = (0..3).map(|_| Stored::of(b"hello")).collect();
    let ctl = controller(&sock, stored);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    at(100);
    tick(&feed, &db, &mut fold);
    assert_eq!(failures(&slack).len(), 1, "the reason shows in the thread");
    at(101);
    tick(&feed, &db, &mut fold);
    let mut fold = replay(&[1], b"hello");
    at(103);
    tick(&feed, &db, &mut fold);
    assert_eq!(failures(&slack).len(), 1, "told once, across a refold");
    assert!(shared(&slack).is_empty());
    slack.upload_down.store(false, Ordering::SeqCst);
    at(110);
    tick(&feed, &db, &mut fold);
    at(111);
    tick(&feed, &db, &mut fold);
    assert_eq!(reads(&seen(ctl)), [1, 1, 1], "no try inside the wait");
    assert_eq!(shared(&slack), ["report.txt"], "shared once");
    assert_eq!(failures(&slack).len(), 1);
}

#[test]
fn a_version_that_keeps_failing_is_given_up_with_one_final_message() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.upload_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, (0..4).map(|_| Stored::of(b"hello")).collect());
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    for t in [100, 103, 699] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(
        failures(&slack).len(),
        1,
        "no final message before ten minutes"
    );
    for t in [710, 800, 5000] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(
        reads(&seen(ctl)),
        [1, 1, 1, 1],
        "four tries, none after the last"
    );
    let told = failures(&slack);
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[1].contains("gave up"), "{}", told[1]);
    assert!(
        told[1].contains("500"),
        "the final message names the reason: {}",
        told[1]
    );
    assert!(!told[0].contains("gave up"));
}

#[test]
fn a_version_given_up_on_is_not_tried_again_after_a_restart() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.upload_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, (0..2).map(|_| Stored::of(b"hello")).collect());
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    for t in [100, 700] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    drop(db);
    slack.upload_down.store(false, Ordering::SeqCst);
    let db = Db::open(&feed.db_path).unwrap();
    let mut fold = replay(&[1], b"hello");
    for t in [701, 3000] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(reads(&seen(ctl)), [1, 1], "no read after the restart");
    assert!(shared(&slack).is_empty(), "never uploaded");
    assert_eq!(failures(&slack).len(), 2);
}

/// Binds a second thread, `C2` `2.0`, with Box `slack-2-1` bound to id 8.
fn other_thread_box(db: &Db) {
    let thread = crate::db::SlackThread {
        channel: "C2".into(),
        thread_ts: "2.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    let row = SlackBox {
        channel: "C2".into(),
        thread_ts: "2.0".into(),
        box_name: BoxName::parse_allow_bare("slack-2-1").unwrap(),
        repo: "acme/app".into(),
        prompt: "fix it".into(),
        base: None,
        provider: None,
        requested_by: "U1".into(),
        parent: None,
        state: crate::db::BoxState::Starting,
    };
    db.insert_starting(&row, 1).unwrap();
    db.bind(&row.box_name, BoxId::new(8), None).unwrap();
}

#[test]
fn a_box_of_another_thread_never_posts_here() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    other_thread_box(&db);
    let mut fold = replay_for(BoxId::new(8), "slack-2-1", &[1], b"hello");
    tick(&feed, &db, &mut fold);
    assert_eq!(reads(&seen(ctl)), [1]);
    let channels: Vec<String> = slack
        .hits("completeuploadexternal")
        .iter()
        .map(|(_, b)| {
            let v: Value = serde_json::from_slice(b).unwrap();
            format!("{}/{}", v["channel_id"], v["thread_ts"])
        })
        .collect();
    assert_eq!(channels, ["\"C2\"/\"2.0\""], "only the Box's own thread");
}

#[test]
fn a_box_with_no_thread_posts_nothing() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay_for(BoxId::new(99), "slack-9-1", &[1], b"hello");
    tick(&feed, &db, &mut fold);
    assert!(slack.hits("completeuploadexternal").is_empty());
    assert!(slack.texts().iter().all(|t| !t.contains("report.txt")));
}

/// An event of `kind` for Box `id` of `slack-2-1`.
fn other_event(cursor: i64, kind: &str, payload: Value) -> Watched {
    Watched::Item(WatchItem::Event(EventRecord {
        cursor,
        box_name: Some("slack-2-1".into()),
        box_id: Some(BoxId::new(8)),
        session: None,
        kind: kind.into(),
        payload,
        ts: 0,
    }))
}

#[test]
fn a_version_waiting_out_its_delay_holds_back_no_other_thread() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::failing("gone"), Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    other_thread_box(&db);
    let mut fold = replay(&[1], b"hello");
    at(100);
    tick(&feed, &db, &mut fold);
    let payload = json!({
        "name": "report.txt", "version": 1, "artifact_kind": "file",
        "content_type": "text/plain", "size": 5, "sha256": sha(b"hello"),
    });
    fold.take(other_event(10, "artifact_published", payload));
    fold.take(other_event(11, "idled", json!({})));
    at(101);
    tick(&feed, &db, &mut fold);
    assert_eq!(shared(&slack), ["report.txt"], "thread B got its file");
    assert!(
        slack
            .texts()
            .iter()
            .any(|t| t.contains("`slack-2-1` finished")),
        "thread B got its finished line: {:?}",
        slack.texts()
    );
    assert_eq!(reads(&seen(ctl)), [1, 1], "thread A waited out its delay");
}

#[test]
fn a_refused_reason_still_schedules_the_next_try_and_is_posted_once_chat_works() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.upload_down.store(true, Ordering::SeqCst);
    slack.post_down.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, (0..5).map(|_| Stored::of(b"hello")).collect());
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    for t in [100, 101, 103] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    slack.post_down.store(false, Ordering::SeqCst);
    for t in [110, 130] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(
        failures(&slack).len(),
        3,
        "the reason is tried on tries 1 to 3 only"
    );
    slack.post_down.store(true, Ordering::SeqCst);
    for t in [800, 900, 5000] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(
        reads(&seen(ctl)),
        [1, 1, 1, 1, 1],
        "five tries, then given up"
    );
}

#[test]
fn a_file_slack_shared_when_the_completion_was_lost_is_not_uploaded_again() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.complete_lost.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    for t in [100, 200, 5000] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(reads(&seen(ctl)), [1], "uploaded once");
    assert_eq!(slack.hits("completeuploadexternal").len(), 1);
    assert!(failures(&slack).is_empty(), "{:?}", slack.texts());
}

#[test]
fn a_file_that_a_lost_completion_did_not_share_is_a_failed_upload() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.complete_lost.store(true, Ordering::SeqCst);
    slack.hide_shares.store(true, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello")]);
    let feed = feed(dir.path(), &slack, &sock);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    at(100);
    tick(&feed, &db, &mut fold);
    assert_eq!(reads(&seen(ctl)), [1]);
    assert_eq!(failures(&slack).len(), 1, "{:?}", slack.texts());
}

#[test]
fn the_wait_between_tries_stops_growing_at_64_seconds() {
    let waits: Vec<i64> = [1, 2, 3, 4, 5, 6, 7, 40].into_iter().map(backoff).collect();
    assert_eq!(waits, [2, 4, 8, 16, 32, 64, 64, 64]);
}

#[test]
fn a_failed_share_check_counts_as_not_shared_and_the_upload_is_tried_again() {
    for mode in [2, 3] {
        let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
        slack.complete_lost.store(true, Ordering::SeqCst);
        slack.info_mode.store(mode, Ordering::SeqCst);
        let sock = dir.path().join("c.sock");
        let ctl = controller(&sock, vec![Stored::of(b"hello"), Stored::of(b"hello")]);
        let feed = feed(dir.path(), &slack, &sock);
        let db = Db::open(&feed.db_path).unwrap();
        bound_box(&db);
        let mut fold = replay(&[1], b"hello");
        for t in [100, 101, 103] {
            at(t);
            tick(&feed, &db, &mut fold);
        }
        assert_eq!(
            reads(&seen(ctl)),
            [1, 1],
            "mode {mode}: tried again after its wait"
        );
        assert_eq!(
            failures(&slack).len(),
            1,
            "mode {mode}: {:?}",
            slack.texts()
        );
    }
}

#[test]
fn a_missing_files_read_scope_is_logged_once_and_the_version_is_retried() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    slack.complete_lost.store(true, Ordering::SeqCst);
    slack.info_mode.store(1, Ordering::SeqCst);
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![Stored::of(b"hello"), Stored::of(b"hello")]);
    let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&lines);
    let log: crate::Log = Arc::new(move |l| sink.lock().unwrap().push(l.to_owned()));
    let feed = feed_logging(dir.path(), &slack, &sock, log);
    let db = Db::open(&feed.db_path).unwrap();
    bound_box(&db);
    let mut fold = replay(&[1], b"hello");
    for t in [100, 103] {
        at(t);
        tick(&feed, &db, &mut fold);
    }
    assert_eq!(reads(&seen(ctl)), [1, 1]);
    let logged = lines.lock().unwrap();
    let scope = logged.iter().filter(|l| l.contains("files:read")).count();
    assert_eq!(scope, 1, "{logged:?}");
    drop(logged);
}

/// An event of `kind` for Box [`ID`] as the controller now names it: `hkfc-verify`, a Box that
/// got the id after the controller's store was recreated.
fn reused(cursor: i64, kind: &str, payload: Value) -> Watched {
    Watched::Item(WatchItem::Event(EventRecord {
        cursor,
        box_name: Some("hkfc-verify".into()),
        box_id: Some(ID),
        session: None,
        kind: kind.into(),
        payload,
        ts: 0,
    }))
}

#[test]
fn a_box_that_reuses_the_rows_id_under_another_name_never_posts_here() {
    let (dir, slack) = (tempfile::tempdir().unwrap(), fake());
    let sock = dir.path().join("c.sock");
    let ctl = controller(&sock, vec![]);
    let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&lines);
    let log: crate::Log = Arc::new(move |l| sink.lock().unwrap().push(l.to_owned()));
    let feed = feed_logging(dir.path(), &slack, &sock, log);
    let db = Db::open(&feed.db_path).unwrap();
    let row = bound_box(&db);
    let mut fold = Fold::default();
    fold.take(Watched::Connected);
    fold.take(reused(1, "working", json!({})));
    fold.take(reused(2, "session_failed", json!({})));
    let published = json!({
        "name": "cms-admin.png", "version": 1, "artifact_kind": "file",
        "content_type": "image/png", "size": 5, "sha256": sha(b"hello"),
    });
    fold.take(reused(3, "artifact_published", published));
    let asked = json!({"question": "which one?", "options": ["a", "b"]});
    fold.take(reused(4, "needs_input", asked));
    fold.take(Watched::Item(WatchItem::CaughtUp { cursor: 4 }));
    tick(&feed, &db, &mut fold);
    tick(&feed, &db, &mut fold);
    assert!(reads(&seen(ctl)).is_empty(), "no stored copy read");
    assert!(slack.hits("").is_empty(), "nothing sent to Slack");
    let state = db
        .fed_boxes()
        .unwrap()
        .into_iter()
        .find(|f| f.boxed.box_name == row.box_name)
        .map(|f| f.boxed.state);
    assert!(
        matches!(state, Some(crate::db::BoxState::Gone { .. })),
        "{state:?}"
    );
    let logged = lines.lock().unwrap();
    let gone_lines = logged.iter().filter(|l| l.contains("hkfc-verify")).count();
    assert_eq!(gone_lines, 1, "{logged:?}");
    drop(logged);
}
