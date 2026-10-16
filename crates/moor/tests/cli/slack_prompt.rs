//! A `needs_input` event posts one prompt with buttons, and answers are routed by owner and freshness.
//!
//! The rig has no agent, so steer refuses every answer; the tests assert that an answer reached
//! steer (its refusal lands in the thread), not that it succeeded. Like the steer tests, each test
//! seeds the store with a working Task bound to the thread and appends events to stand in for progress.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use serde_json::Value;

use super::slack::{Rig, CHANNEL, QUIET, TASK, TS, USER};
use super::slack_fake::{Click, Post};

/// A second user who is not the Task owner.
const OTHER: &str = "U0OTHER";
/// A question with two options.
const PAYLOAD: &str = r#"{"question":"Proceed?","options":["Yes","No"]}"#;

/// The store the controller shares with the test.
struct Seed {
    db: Connection,
    task: i64,
}

impl Seed {
    /// Opens the store once the controller has created both tables.
    fn open_ready(path: &Path) -> Option<Connection> {
        let db = Connection::open(path).ok()?;
        db.busy_timeout(Duration::from_secs(5)).unwrap();
        let ready = db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('tasks', 'slack_threads')",
            [],
            |row| row.get::<_, i64>(0),
        );
        ready.is_ok_and(|n| n == 2).then_some(db)
    }

    /// Inserts a working Task bound to the thread `TS` and waits for its status message.
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
            "INSERT INTO tasks (name, repo, state, created_at, updated_at) VALUES (?1, 'demo', 'working', 0, 0)",
            [TASK],
        )
        .unwrap();
        let task = db.last_insert_rowid();
        db.execute(
            "INSERT INTO events (task, kind, payload, ts) VALUES (?1, 'working', '{}', 0)",
            [task],
        )
        .unwrap();
        db.execute(
            "INSERT INTO slack_threads (channel, thread_ts, task, owner, created_at) VALUES (?1, ?2, ?3, ?4, 0)",
            [CHANNEL, TS, TASK, USER],
        )
        .unwrap();
        rig.fake.wait_posts(1);
        Self { db, task }
    }

    /// Appends a Task event of `kind` with a JSON `payload`.
    fn event(&self, kind: &str, payload: &str) {
        self.db
            .execute(
                "INSERT INTO events (task, kind, payload, ts) VALUES (?1, ?2, ?3, 0)",
                (self.task, kind, payload),
            )
            .unwrap();
    }
}

/// The `action_id` of every button in `blocks`.
fn buttons(blocks: &Value) -> Vec<String> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "actions")
        .flat_map(|b| b["elements"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| e["action_id"].as_str().map(str::to_owned))
        .collect()
}

/// Presses button `choice` on `prompt` as `user`.
fn click(rig: &Rig, n: u32, user: &str, prompt: &Post, choice: u32) {
    rig.fake.send_click(&Click {
        envelope_id: &format!("env-click-{n}"),
        channel: CHANNEL,
        user,
        message_ts: &prompt.ts,
        thread_ts: TS,
        value: &choice.to_string(),
        action_id: &format!("answer_{choice}"),
    });
}

/// Starts a rig with a Task that already asked `PAYLOAD`; returns the prompt post.
fn asked() -> (Rig, Seed, Post) {
    let rig = Rig::start();
    let seed = Seed::bind(&rig);
    seed.event("needs_input", PAYLOAD);
    let prompt = rig.fake.wait_posts(2).remove(1);
    (rig, seed, prompt)
}

#[test]
fn a_needs_input_event_posts_one_prompt_with_buttons_for_the_owner() {
    let (rig, seed, prompt) = asked();
    assert_eq!(prompt.thread_ts, TS);
    assert_eq!(prompt.channel, CHANNEL);
    assert!(
        prompt.text.contains(&format!("<@{USER}>")),
        "{}",
        prompt.text
    );
    assert!(prompt.text.contains("Proceed?"), "{}", prompt.text);
    assert_eq!(buttons(&prompt.blocks), ["answer_1", "answer_2"]);

    // Several poll periods pass without a duplicate.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(rig.fake.posts().len(), 2, "{:?}", rig.fake.posts());

    seed.event("needs_input", r#"{"question":"Again?","options":["A"]}"#);
    let posts = rig.fake.wait_posts(3);
    assert!(posts[2].text.contains("Again?"), "{}", posts[2].text);
    assert_eq!(buttons(&posts[2].blocks), ["answer_1"]);
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.posts().len(), 3, "{:?}", rig.fake.posts());
}

#[test]
fn an_owner_click_on_a_live_prompt_reaches_steer() {
    let (rig, _seed, prompt) = asked();
    click(&rig, 1, USER, &prompt, 2);
    let posts = rig.fake.wait_posts(3);
    // No agent runs in the rig, so steer refuses; its refusal in the thread shows the click was
    // routed through the menu path rather than refused as stale or foreign.
    let outcome = &posts[2].text;
    assert!(!outcome.contains("no longer open"), "{outcome}");
    assert!(outcome.contains(TASK), "{outcome}");
    assert_eq!(posts[2].thread_ts, TS);
    assert!(rig.fake.ephemerals().is_empty());
    assert!(rig.fake.updates().is_empty(), "{:?}", rig.fake.updates());
}

#[test]
fn a_click_from_another_user_gets_an_ephemeral_refusal() {
    let (rig, _seed, prompt) = asked();
    click(&rig, 1, OTHER, &prompt, 1);
    let ephemerals = rig.fake.wait_ephemerals(1);
    assert_eq!(ephemerals[0].user, OTHER);
    assert_eq!(ephemerals[0].channel, CHANNEL);
    assert_eq!(ephemerals[0].thread_ts, TS);
    assert_eq!(
        ephemerals[0].text,
        format!("Only <@{USER}> can answer this prompt.")
    );
    std::thread::sleep(QUIET);
    assert_eq!(rig.fake.posts().len(), 2, "{:?}", rig.fake.posts());
    assert!(rig.fake.updates().is_empty(), "{:?}", rig.fake.updates());
}

#[test]
fn a_click_after_the_agent_moved_on_is_refused_with_its_state() {
    let (rig, seed, prompt) = asked();
    seed.event("working", "{}");
    click(&rig, 1, USER, &prompt, 1);
    let posts = rig.fake.wait_posts(3);
    let outcome = &posts[2].text;
    assert!(
        outcome.contains("This prompt is no longer open"),
        "{outcome}"
    );
    assert!(outcome.contains("is now working."), "{outcome}");
    assert!(!outcome.contains("no live agent session"), "{outcome}");
    assert_eq!(posts[2].thread_ts, TS);
    // The prompt message is left as it was: no edit carries buttons.
    assert!(rig
        .fake
        .updates()
        .iter()
        .all(|u| buttons(&u.blocks).is_empty()));
}

#[test]
fn a_digit_reply_from_the_owner_reaches_steer() {
    let (rig, _seed, _prompt) = asked();
    rig.fake.send_reply(&super::slack_fake::Reply {
        event_id: "Ev1",
        envelope_id: "env-1",
        channel: CHANNEL,
        user: USER,
        text: "2",
        ts: "1730000100.000001",
        thread_ts: TS,
        subtype: None,
        bot_id: None,
    });
    let posts = rig.fake.wait_posts(3);
    assert!(posts[2].text.contains(TASK), "{}", posts[2].text);
    assert_eq!(posts[2].thread_ts, TS);
}
