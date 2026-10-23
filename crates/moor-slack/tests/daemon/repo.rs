//! `/moor repo` against the daemon: the ack and the rows in `slack.db`.

#![allow(clippy::indexing_slicing, reason = "test code")]

use super::support::{Rig, CHANNEL, QUIET, USER};

/// Runs `/moor <text>` as `user` in a channel and returns the ack's text.
fn run(rig: &Rig, envelope_id: &str, user: &str, text: &str) -> String {
    rig.slack.send_slash(envelope_id, CHANNEL, user, text);
    let payload = rig.slack.wait_ack_payload(envelope_id);
    payload["text"].as_str().unwrap().to_owned()
}

/// `(channel, repo, added_by, is_default)` of every channel repo in the daemon's `slack.db`.
fn rows(rig: &Rig) -> Vec<(String, String, String, bool)> {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    let mut stmt = db
        .prepare(
            "SELECT channel, repo, added_by, is_default FROM slack_channel_repos \
             ORDER BY channel, repo",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap();
    rows.map(Result::unwrap).collect()
}

#[test]
fn an_added_repo_is_recorded_before_the_ack_and_posts_nothing() {
    let (rig, _controller) = Rig::start();
    let text = run(&rig, "env-1", USER, "repo add acme/api");
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    let want = (
        CHANNEL.to_owned(),
        "acme/api".to_owned(),
        USER.to_owned(),
        false,
    );
    assert_eq!(rows(&rig), [want]);
    std::thread::sleep(QUIET);
    assert!(rig.slack.posts().is_empty(), "{:?}", rig.slack.posts());
}

#[test]
fn a_repo_that_is_not_owner_name_is_refused_and_not_recorded() {
    let (rig, _controller) = Rig::start();
    let text = run(&rig, "env-1", USER, "repo add acme");
    assert_eq!(
        text,
        "`acme` is not a GitHub repo name. Use `/moor repo add <owner/name>`."
    );
    assert!(rows(&rig).is_empty());
}

#[test]
fn a_channel_works_on_several_repos_with_one_default() {
    let (rig, _controller) = Rig::start();
    run(&rig, "env-1", USER, "repo add acme/api");
    run(&rig, "env-2", USER, "repo add acme/web");
    run(&rig, "env-3", USER, "repo default acme/api");
    let text = run(&rig, "env-4", USER, "repo default acme/web");
    assert_eq!(text, "Recorded. `acme/web` is this channel's default repo.");
    let text = run(&rig, "env-5", USER, "repo");
    assert_eq!(
        text,
        "This channel works on `acme/api`, `acme/web` (default)."
    );
    run(&rig, "env-6", USER, "repo remove acme/web");
    let text = run(&rig, "env-7", USER, "repo");
    assert_eq!(text, "This channel works on `acme/api`.");
    let want = (
        CHANNEL.to_owned(),
        "acme/api".to_owned(),
        USER.to_owned(),
        false,
    );
    assert_eq!(rows(&rig), [want]);
}

#[test]
fn a_user_nobody_listed_changes_the_channels_repos_like_anyone() {
    let (rig, _controller) = Rig::start();
    let text = run(&rig, "env-1", "U0OTHER", "repo add acme/api");
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    let want = (
        CHANNEL.to_owned(),
        "acme/api".to_owned(),
        "U0OTHER".to_owned(),
        false,
    );
    assert_eq!(rows(&rig), [want]);
}

#[test]
fn an_add_that_slack_db_cannot_record_gets_no_ack_until_it_can() {
    let (rig, _controller) = Rig::start();
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.execute_batch("ALTER TABLE slack_channel_repos RENAME TO hidden")
        .unwrap();
    rig.slack
        .send_slash("env-1", CHANNEL, USER, "repo add acme/api");
    rig.wait_stderr("Slack sends the envelope again");
    std::thread::sleep(QUIET);
    assert!(rig.slack.acks().is_empty(), "{:?}", rig.slack.acks());
    db.execute_batch("ALTER TABLE hidden RENAME TO slack_channel_repos")
        .unwrap();
    let text = run(&rig, "env-1", USER, "repo add acme/api");
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    assert_eq!(rows(&rig).len(), 1);
}
