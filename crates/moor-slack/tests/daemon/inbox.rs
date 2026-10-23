//! Messages in a thread the front keeps: the starter's are queued before the ack, anyone
//! else's get one line and are not queued.

#![allow(clippy::indexing_slicing, reason = "test code")]

use super::support::controller::FakeController;
use super::support::slack_fake::Reply;
use super::support::{Rig, CHANNEL, QUIET, TS, USER};

/// Another person on the workspace.
pub(super) const OTHER: &str = "U0OTHER";

/// Opens thread [`TS`] with a mention and waits for its turn's line; the config names no
/// coordinator binary, so the turn says the coordinator is unavailable.
pub(super) fn started_thread() -> (Rig, FakeController) {
    let (rig, controller) = Rig::start();
    rig.mention(1, USER, "<@UBOT> fix the thing", TS, None);
    rig.slack.wait_posts(1);
    (rig, controller)
}

/// Pushes a plain reply in thread [`TS`] from `user`, with ids derived from `n`.
pub(super) fn reply(rig: &Rig, n: u32, user: &str, text: &str) {
    rig.slack.send_reply(&Reply {
        event_id: &format!("Ev{n}"),
        envelope_id: &format!("env-{n}"),
        channel: CHANNEL,
        user,
        text,
        ts: &format!("1730000100.{n:06}"),
        thread_ts: TS,
        subtype: None,
        bot_id: None,
    });
}

/// `(kind, author, text, source)` of every inbox item in the daemon's `slack.db`, in the
/// order they were recorded.
pub(super) fn inbox(rig: &Rig) -> Vec<(String, String, String, String)> {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let mut stmt = db
        .prepare("SELECT kind, author, text, source FROM slack_inbox ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

/// An inbox row for a message from the starter.
fn message(text: &str, source: &str) -> (String, String, String, String) {
    (
        "message".to_owned(),
        USER.to_owned(),
        text.to_owned(),
        source.to_owned(),
    )
}

#[test]
fn the_starters_messages_are_queued_in_the_order_they_arrive() {
    let (rig, _controller) = started_thread();
    reply(&rig, 2, USER, "use tokio");
    reply(&rig, 3, USER, "and keep the old API");
    rig.mention(
        4,
        USER,
        "<@UBOT> also add a test",
        "1730000200.000400",
        Some(TS),
    );
    rig.slack.wait_acks(4);
    let want = [
        message("fix the thing", "Ev1"),
        message("use tokio", "Ev2"),
        message("and keep the old API", "Ev3"),
        message("also add a test", "Ev4"),
    ];
    assert_eq!(inbox(&rig), want);
}

#[test]
fn another_persons_message_is_refused_and_not_queued() {
    let (rig, _controller) = started_thread();
    reply(&rig, 2, OTHER, "use tokio");
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].thread_ts, TS);
    assert_eq!(
        posts[1].text,
        format!("Only <@{USER}> can direct this thread.")
    );
    assert_eq!(inbox(&rig), [message("fix the thing", "Ev1")]);
}

#[test]
fn an_acked_message_survives_a_crash_and_its_redelivery_is_queued_once() {
    let (mut rig, _controller) = started_thread();
    reply(&rig, 2, USER, "use tokio");
    rig.slack.wait_acks(2);
    rig.kill_daemon();
    assert_eq!(
        inbox(&rig),
        [message("fix the thing", "Ev1"), message("use tokio", "Ev2")]
    );
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.slack.send_reply(&Reply {
        event_id: "Ev2",
        envelope_id: "env-2-retry",
        channel: CHANNEL,
        user: USER,
        text: "use tokio",
        ts: "1730000100.000002",
        thread_ts: TS,
        subtype: None,
        bot_id: None,
    });
    rig.slack.wait_acks(3);
    assert_eq!(
        inbox(&rig),
        [message("fix the thing", "Ev1"), message("use tokio", "Ev2")]
    );
}

#[test]
fn a_message_that_fails_to_record_is_not_acked_and_its_redelivery_is_queued_once() {
    let (rig, _controller) = started_thread();
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER no_items BEFORE INSERT ON slack_inbox \
         BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
    )
    .unwrap();
    reply(&rig, 2, USER, "use tokio");
    std::thread::sleep(QUIET);
    assert!(
        !rig.slack.acks().iter().any(|id| id == "env-2"),
        "acked before it was recorded: {:?}",
        rig.slack.acks()
    );
    assert_eq!(inbox(&rig), [message("fix the thing", "Ev1")]);
    db.execute_batch("DROP TRIGGER no_items;").unwrap();
    let acked = rig.slack.acks().len();
    reply(&rig, 2, USER, "use tokio");
    rig.slack.wait_acks(acked + 1);
    assert!(rig.slack.acks().iter().any(|id| id == "env-2"));
    assert_eq!(
        inbox(&rig),
        [message("fix the thing", "Ev1"), message("use tokio", "Ev2")]
    );
    reply(&rig, 2, USER, "use tokio");
    rig.slack.wait_acks(acked + 2);
    assert_eq!(
        inbox(&rig),
        [message("fix the thing", "Ev1"), message("use tokio", "Ev2")]
    );
}
