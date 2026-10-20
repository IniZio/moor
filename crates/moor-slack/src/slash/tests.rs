//! Tests for `/moor repo`: what it records before the ack and what the repo check posts.
#![allow(
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unnecessary_wraps,
    reason = "test code; helpers build controller answers"
)]

use std::cell::RefCell;

use moor_client::ClientError;
use moor_core::request::Request;
use moor_core::response::Response;
use serde_json::json;

use super::{check_repo, parse, record, RepoCmd};
use crate::command::Allow;
use crate::db::Db;
use crate::socket::Slash;
use crate::worker::{Fx, Job};

fn slash(envelope_id: &str, user: &str, channel: &str, text: &str) -> Slash {
    Slash {
        envelope_id: envelope_id.into(),
        channel: channel.into(),
        user: user.into(),
        text: text.into(),
        trigger_id: None,
    }
}

/// Records `s` with user `U1` and channel `C1` allowed; returns the ack text and the job.
fn run(db: &Db, s: &Slash) -> (String, Option<Job>) {
    let channels = vec!["C1".to_owned()];
    let users = vec!["U1".to_owned()];
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    let state = std::path::Path::new("/nonexistent");
    let (kept, job) = record(s, db, &allow, state, 1_000, &|_| {}).unwrap();
    assert_eq!(kept.then, None);
    let ack = kept.ack.unwrap();
    (ack["text"].as_str().unwrap().to_owned(), job)
}

fn db(dir: &std::path::Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

#[test]
fn parses_each_repo_command() {
    assert_eq!(parse("repo"), Some(RepoCmd::List));
    assert_eq!(
        parse(" Repo  ADD acme/api "),
        Some(RepoCmd::Add("acme/api"))
    );
    assert_eq!(
        parse("repo remove a.b_c-d"),
        Some(RepoCmd::Remove("a.b_c-d"))
    );
    assert_eq!(
        parse("repo default acme/api"),
        Some(RepoCmd::Default("acme/api"))
    );
    for bad in [
        "",
        "repos",
        "repo add",
        "repo acme/api",
        "repo add acme/api extra",
        "repo add <!channel>",
        "repo add <@U1>",
        "login",
    ] {
        assert_eq!(parse(bad), None, "{bad}");
    }
}

#[test]
fn add_records_the_repo_and_its_adder_before_the_ack() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (text, job) = run(&db, &slash("e1", "U1", "C1", "repo add acme/api"));
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    let want = Job::CheckRepo {
        channel: "C1".into(),
        repo: "acme/api".into(),
    };
    assert_eq!(job, Some(want));
    let rows = db.channel_repos("C1").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].repo.as_str(), rows[0].added_by.as_str()),
        ("acme/api", "U1")
    );
}

#[test]
fn a_redelivered_add_queues_no_second_check() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    run(&db, &slash("e1", "U1", "C1", "repo add acme/api"));
    db.remove_channel_repo("C1", "acme/api").unwrap();
    let (_, job) = run(&db, &slash("e1", "U1", "C1", "repo add acme/api"));
    assert_eq!(job, None);
}

#[test]
fn list_marks_the_default_and_remove_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (text, _) = run(&db, &slash("e0", "U1", "C1", "repo"));
    assert_eq!(
        text,
        "This channel has no repos. Add one with `/moor repo add <owner/name>`."
    );
    run(&db, &slash("e1", "U1", "C1", "repo add acme/web"));
    run(&db, &slash("e2", "U1", "C1", "repo add acme/api"));
    let (text, _) = run(&db, &slash("e3", "U1", "C1", "repo default acme/web"));
    assert_eq!(text, "Recorded. `acme/web` is this channel's default repo.");
    let (text, _) = run(&db, &slash("e4", "U1", "C1", "repo"));
    assert_eq!(
        text,
        "This channel works on `acme/api`, `acme/web` (default)."
    );
    let (text, _) = run(&db, &slash("e5", "U1", "C1", "repo default acme/api"));
    assert_eq!(text, "Recorded. `acme/api` is this channel's default repo.");
    let (text, _) = run(&db, &slash("e6", "U1", "C1", "repo"));
    assert_eq!(
        text,
        "This channel works on `acme/api` (default), `acme/web`."
    );
    let (text, _) = run(&db, &slash("e7", "U1", "C1", "repo remove acme/api"));
    assert_eq!(text, "Removed `acme/api` from this channel.");
    let (text, _) = run(&db, &slash("e8", "U1", "C1", "repo"));
    assert_eq!(text, "This channel works on `acme/web`.");
}

#[test]
fn a_repo_the_channel_lacks_is_named_and_nothing_changes() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    for (n, text) in ["repo remove acme/api", "repo default acme/api"]
        .into_iter()
        .enumerate()
    {
        let (reply, job) = run(&db, &slash(&format!("e{n}"), "U1", "C1", text));
        assert_eq!(reply, "`acme/api` is not one of this channel's repos.");
        assert_eq!(job, None);
    }
    assert!(db.channel_repos("C1").unwrap().is_empty());
}

#[test]
fn a_user_or_channel_off_the_lists_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (text, job) = run(&db, &slash("e1", "U9", "C1", "repo add acme/api"));
    assert_eq!(text, "<@U9> is not allowed to change this channel's repos.");
    assert_eq!(job, None);
    let (text, job) = run(&db, &slash("e2", "U1", "C9", "repo add acme/api"));
    assert_eq!(text, "moor does not work in this channel.");
    assert_eq!(job, None);
    assert!(db.channel_repos("C1").unwrap().is_empty());
    assert!(db.channel_repos("C9").unwrap().is_empty());
}

/// Runs the repo check of `repo` against a controller that answers `repos` with `answer`;
/// returns the channel lines and the log lines.
fn checked(
    repo: &str,
    answer: Result<Response, ClientError>,
) -> (Vec<(String, String)>, Vec<String>) {
    let said = RefCell::new(Vec::new());
    let logged = RefCell::new(Vec::new());
    let mut answer = Some(answer);
    let mut call = |req: Request| {
        assert!(matches!(req, Request::Repos(_)), "{req:?}");
        answer.take().unwrap()
    };
    let mut reply = |_: &str, _: &str, _: &str| panic!("the check posts no thread reply");
    let mut say = |c: &str, t: &str| said.borrow_mut().push((c.to_owned(), t.to_owned()));
    let log = |line: &str| logged.borrow_mut().push(line.to_owned());
    let mut check_token = |_| panic!("the check checks no token");
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        say: &mut say,
        check_token: &mut check_token,
        log: &log,
    };
    check_repo(&mut fx, "C1", repo);
    (said.into_inner(), logged.into_inner())
}

fn listed(names: &[(&str, &str)]) -> Result<Response, ClientError> {
    let repos: Vec<_> = names
        .iter()
        .map(|(name, url)| json!({"name": name, "url": url}))
        .collect();
    Ok(Response::ok("repos", Some(json!({ "repos": repos }))))
}

#[test]
fn a_repo_the_controller_knows_by_name_or_url_posts_nothing() {
    let known = [("acme/api", "https://git.example/acme/api.git")];
    for repo in ["acme/api", "https://git.example/acme/api"] {
        assert_eq!(checked(repo, listed(&known)), (vec![], vec![]), "{repo}");
    }
}

#[test]
fn an_unknown_repo_gets_one_line_naming_the_known_ones() {
    let known = [("acme/api", "u1"), ("acme/web", "u2")];
    let (said, _) = checked("acme/app", listed(&known));
    let line = "The controller does not know `acme/app`. Known repos: `acme/api`, `acme/web`.";
    assert_eq!(said, [("C1".to_owned(), line.to_owned())]);
    let (said, _) = checked("acme/app", listed(&[]));
    let line = "The controller does not know `acme/app`. It knows no repos.";
    assert_eq!(said, [("C1".to_owned(), line.to_owned())]);
}

#[test]
fn a_controller_that_cannot_answer_is_logged_and_nothing_is_posted() {
    let hung_up = Err(ClientError::Io(std::io::ErrorKind::ConnectionReset.into()));
    let (said, logged) = checked("acme/app", hung_up);
    assert!(said.is_empty());
    assert_eq!(logged.len(), 1);
    assert!(
        logged[0].starts_with("repo check of acme/app: "),
        "{logged:?}"
    );
}
