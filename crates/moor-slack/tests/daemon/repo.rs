//! `/moor repo` against the daemon: the ack, the rows in `slack.db` and the repo check.

#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::support::controller::{FakeController, Scripted};
use super::support::{Rig, CHANNEL, QUIET, USER};

/// Scripts one `repos` answer that lists `names`.
fn script_repos(controller: &FakeController, names: &[&str]) {
    let repos: Vec<_> = names
        .iter()
        .map(|n| json!({"name": n, "url": format!("https://git.example/{n}.git")}))
        .collect();
    let value = json!({ "repos": repos });
    controller.script("repos", Scripted::Reply(Response::ok("repos", Some(value))));
}

/// Runs `/moor <text>` as `user` in the allowed channel and returns the ack's text.
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

fn repos_requests(controller: &FakeController) -> usize {
    controller.seen(|seen| {
        seen.requests
            .iter()
            .filter(|r| matches!(r, Request::Repos(_)))
            .count()
    })
}

#[test]
fn an_added_repo_is_recorded_before_the_ack_and_a_known_one_posts_nothing() {
    let (rig, controller) = Rig::start();
    script_repos(&controller, &["acme/api"]);
    let text = run(&rig, "env-1", USER, "repo add acme/api");
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    let want = (
        CHANNEL.to_owned(),
        "acme/api".to_owned(),
        USER.to_owned(),
        false,
    );
    assert_eq!(rows(&rig), [want]);
    controller.wait_until("the repo check", |seen| !seen.requests.is_empty());
    std::thread::sleep(QUIET);
    assert!(rig.slack.posts().is_empty(), "{:?}", rig.slack.posts());
}

#[test]
fn an_unknown_repo_gets_one_channel_line_naming_the_known_repos() {
    let (rig, controller) = Rig::start();
    script_repos(&controller, &["acme/api", "acme/web"]);
    run(&rig, "env-1", USER, "repo add acme/app");
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].channel, CHANNEL);
    assert_eq!(posts[0].thread_ts, "", "posted at the top level");
    let line = "The controller does not know `acme/app`. Known repos: `acme/api`, `acme/web`.";
    assert_eq!(posts[0].text, line);
    assert_eq!(rows(&rig).len(), 1, "the repo stays recorded");
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 1);
}

#[test]
fn a_channel_works_on_several_repos_with_one_default() {
    let (rig, controller) = Rig::start();
    script_repos(&controller, &["acme/api", "acme/web"]);
    script_repos(&controller, &["acme/api", "acme/web"]);
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
fn a_user_off_the_list_is_refused_and_nothing_is_recorded() {
    let (rig, controller) = Rig::start();
    let text = run(&rig, "env-1", "U0OTHER", "repo add acme/api");
    assert_eq!(
        text,
        "<@U0OTHER> is not allowed to change this channel's repos."
    );
    std::thread::sleep(QUIET);
    assert!(rows(&rig).is_empty());
    assert_eq!(repos_requests(&controller), 0);
    assert!(rig.slack.posts().is_empty());
}

#[test]
fn a_redelivered_add_is_checked_once() {
    let (rig, controller) = Rig::start();
    script_repos(&controller, &[]);
    run(&rig, "env-1", USER, "repo add acme/app");
    rig.slack.wait_posts(1);
    rig.slack
        .send_slash("env-1", CHANNEL, USER, "repo add acme/app");
    rig.slack.wait_acks(2);
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 1);
    assert_eq!(repos_requests(&controller), 1);
}
