//! Thread replies steer and close a Task, and one status message follows it, against the fake Slack.
//!
//! The rig has no box provider, so a Task cannot be started from Slack here. Each test seeds the
//! controller's store with an open Task and a bound thread instead, then drives the real Slack
//! handlers; events are appended to the store to stand in for the Task's progress.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use super::slack::{Rig, CHANNEL, QUIET, TASK, TS, USER};
use super::slack_fake::{Post, Reply, Update, APP_TOKEN, BOT_TOKEN};

/// A second user who is not the Task owner.
const OTHER: &str = "U0OTHER";

/// The store the controller shares with the test.
struct Seed {
    db: Connection,
    task: i64,
    /// The status message the controller posted for the thread.
    status: Post,
}

impl Seed {
    /// Opens the store once the controller has created both tables.
    fn open_ready(path: &Path) -> Option<Connection> {
        let db = Connection::open(path).ok()?;
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let ready = db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('boxes', 'bindings')",
            [],
            |row| row.get::<_, i64>(0),
        );
        ready.is_ok_and(|n| n == 2).then_some(db)
    }

    /// Waits for the controller's store, inserts a working Task bound to the thread `TS`, and waits
    /// for its status message so later posts are all replies.
    fn bind(rig: &Rig) -> Self {
        let path = rig.root.join("state").join("moor.db");
        let db = (0..200)
            .find_map(|_| {
                let db = Self::open_ready(&path);
                if db.is_none() {
                    std::thread::sleep(Duration::from_millis(50));
                }
                db
            })
            .expect("controller never created its store");
        db.execute(
            "INSERT INTO boxes (name, repo, state, created_at, updated_at) VALUES (?1, 'demo', 'ready', 0, 0)",
            [TASK],
        )
        .unwrap();
        let task = db.last_insert_rowid();
        db.execute(
            "INSERT INTO sessions (box, ordinal, state, started_at) VALUES (?1, 1, 'working', 0)",
            [task],
        )
        .unwrap();
        let session = db.last_insert_rowid();
        db.execute(
            "INSERT INTO events (box, kind, payload, ts) VALUES (?1, 'working', '{}', 0)",
            [task],
        )
        .unwrap();
        db.execute(
            "INSERT INTO bindings (front, conv_key, session, owner, created_at) VALUES ('slack', ?1, ?2, ?3, 0)",
            rusqlite::params![format!("{CHANNEL}/{TS}"), session, USER],
        )
        .unwrap();
        let status = rig.fake.wait_posts(1).remove(0);
        Self { db, task, status }
    }

    /// Appends a Task event of `kind` with a JSON `payload`.
    fn event(&self, kind: &str, payload: &str) {
        self.db
            .execute(
                "INSERT INTO events (box, kind, payload, ts) VALUES (?1, ?2, ?3, 0)",
                (self.task, kind, payload),
            )
            .unwrap();
    }
}

/// Pushes a thread reply into the bound thread.
fn reply(rig: &Rig, n: u32, user: &str, text: &str, subtype: Option<&str>, bot_id: Option<&str>) {
    rig.fake.send_reply(&Reply {
        event_id: &format!("Ev{n}"),
        envelope_id: &format!("env-{n}"),
        channel: CHANNEL,
        user,
        text,
        ts: &format!("1730000100.{n:06}"),
        thread_ts: TS,
        subtype,
        bot_id,
    });
}

/// A loopback port that is free right now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The posts after the status message.
fn replies(posts: &[Post]) -> Vec<&Post> {
    posts.iter().skip(1).collect()
}

#[test]
fn owner_reply_steers_the_task() {
    let rig = Rig::start();
    let _seed = Seed::bind(&rig);
    reply(&rig, 1, USER, "also add tests", None, None);
    let (posts, _) = rig
        .fake
        .wait_until("the steer outcome", |p, _| p.len() >= 2);
    // No agent runs in the rig, so the Task refuses the steer; the refusal reaching the thread
    // verbatim shows the text was routed to the Task for its owner.
    let outcome = &replies(&posts)[0].text;
    assert!(outcome.contains("no live agent session"), "{outcome}");
    assert!(outcome.contains(TASK), "{outcome}");
    assert_eq!(posts[1].thread_ts, TS);
}

#[test]
fn reply_from_another_user_is_refused_with_an_owner_mention() {
    let rig = Rig::start();
    let _seed = Seed::bind(&rig);
    reply(&rig, 1, OTHER, "stop that", None, None);
    reply(&rig, 2, OTHER, "close", None, None);
    let (posts, _) = rig.fake.wait_until("two refusals", |p, _| p.len() >= 3);
    for refusal in replies(&posts) {
        assert_eq!(refusal.text, format!("Only <@{USER}> can steer this Task."));
        assert_eq!(refusal.thread_ts, TS);
    }
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.posts().len(), 3, "{:?}", rig.fake.posts());
}

#[test]
fn bots_edits_joins_and_unbound_threads_are_ignored() {
    let rig = Rig::start();
    let _seed = Seed::bind(&rig);
    reply(&rig, 1, USER, "from a bot", None, Some("B0BOT"));
    reply(&rig, 2, USER, "edited", Some("message_changed"), None);
    reply(&rig, 3, USER, "joined", Some("channel_join"), None);
    rig.fake.send_reply(&Reply {
        event_id: "Ev4",
        envelope_id: "env-4",
        channel: CHANNEL,
        user: USER,
        text: "unbound thread",
        ts: "1730000200.000100",
        thread_ts: "1730000199.000100",
        subtype: None,
        bot_id: None,
    });
    rig.fake.send_reply(&Reply {
        event_id: "Ev5",
        envelope_id: "env-5",
        channel: CHANNEL,
        user: USER,
        text: "not even a reply",
        ts: TS,
        thread_ts: TS,
        subtype: None,
        bot_id: None,
    });
    assert_eq!(rig.fake.wait_acks(5).len(), 5);
    std::thread::sleep(QUIET);
    let posts = rig.fake.posts();
    assert_eq!(
        posts.len(),
        1,
        "only the status message expected: {posts:?}"
    );
}

#[test]
fn close_in_the_thread_closes_the_task() {
    let rig = Rig::start();
    let _seed = Seed::bind(&rig);
    reply(&rig, 1, USER, "close", None, None);
    let updates = rig.fake.wait_updates(1);
    assert!(updates[0].text.contains("closed"), "{}", updates[0].text);
    let shown = rig.show(TASK);
    assert_eq!(shown["value"]["task"]["state"], "closed", "{shown}");
    assert_eq!(rig.fake.posts().len(), 1, "{:?}", rig.fake.posts());
}

#[test]
fn a_close_the_task_refuses_is_posted_verbatim() {
    let rig = Rig::start();
    let seed = Seed::bind(&rig);
    seed.db
        .execute(
            "UPDATE boxes SET state = 'closed' WHERE id = ?1",
            [seed.task],
        )
        .unwrap();
    seed.db
        .execute(
            "UPDATE sessions SET state = 'ended' WHERE box = ?1",
            [seed.task],
        )
        .unwrap();
    reply(&rig, 1, USER, "close", None, None);
    let (posts, _) = rig.fake.wait_until("the close error", |p, _| p.len() >= 2);
    assert_eq!(posts[1].text, "task is already closed");
    assert_eq!(posts[1].thread_ts, TS);
}

fn assert_edits_target(updates: &[Update], status: &Post) {
    for update in updates {
        assert_eq!(update.ts, status.ts);
        assert_eq!(update.channel, CHANNEL);
    }
}

#[test]
fn one_status_message_is_posted_then_edited_through_the_task_life() {
    let rig = Rig::start();
    let seed = Seed::bind(&rig);
    let status = seed.status.clone();
    assert_eq!(status.thread_ts, TS);
    assert!(status.text.contains("working"), "{}", status.text);
    assert!(status.text.contains(TASK), "{}", status.text);

    seed.event("idled", "{}");
    let updates = rig.fake.wait_updates(1);
    assert!(updates[0].text.contains("idle"), "{}", updates[0].text);

    seed.event("needs_input", r#"{"question":"which database?"}"#);
    let updates = rig.fake.wait_updates(2);
    assert!(
        updates[1].text.contains("needs input"),
        "{}",
        updates[1].text
    );
    assert!(
        updates[1].text.contains("which database?"),
        "{}",
        updates[1].text
    );

    seed.event("closed", "{}");
    let updates = rig.fake.wait_updates(3);
    assert!(updates[2].text.contains("closed"), "{}", updates[2].text);

    assert_edits_target(&updates, &status);
    // The needs_input event also posts a separate prompt; only the status is edited.
    let posts = rig.fake.posts();
    let status_posts = posts
        .iter()
        .filter(|p| !p.text.contains("which database?"))
        .count();
    assert_eq!(status_posts, 1, "{posts:?}");
}

#[test]
fn rapid_changes_coalesce_into_one_edit_per_window() {
    let rig = Rig::start();
    let seed = Seed::bind(&rig);
    seed.event("idled", "{}");
    seed.event("working", "{}");
    seed.event("idled", "{}");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(rig.fake.updates().is_empty(), "{:?}", rig.fake.updates());
    let updates = rig.fake.wait_updates(1);
    assert!(updates[0].text.contains("idle"), "{}", updates[0].text);
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.updates().len(), 1, "{:?}", rig.fake.updates());
}

#[test]
fn status_links_the_web_ui_when_configured() {
    let port = free_port();
    let rig = Rig::start_with(Some(port));
    let seed = Seed::bind(&rig);
    let link = format!("http://127.0.0.1:{port}/");
    assert!(seed.status.text.contains(&link), "{}", seed.status.text);
}

#[test]
fn tokens_never_appear_in_posted_text_or_stderr() {
    let mut rig = Rig::start_with(Some(free_port()));
    let seed = Seed::bind(&rig);
    reply(&rig, 1, USER, "also add tests", None, None);
    reply(&rig, 2, OTHER, "stop", None, None);
    seed.event("closed", "{}");
    rig.fake
        .wait_until("replies and an edit", |p, u| p.len() >= 3 && !u.is_empty());
    let stderr = rig.stderr();
    let texts: Vec<String> = rig
        .fake
        .posts()
        .iter()
        .map(|p| p.text.clone())
        .chain(rig.fake.updates().iter().map(|u: &Update| u.text.clone()))
        .chain([stderr])
        .collect();
    for text in texts {
        for token in [APP_TOKEN, BOT_TOKEN, "web-secret"] {
            assert!(!text.contains(token), "{token} leaked in {text}");
        }
    }
}
