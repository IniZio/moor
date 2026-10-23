//! Tests for `/moor repo`: what it records before the ack.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use super::{parse, record, RepoCmd};
use crate::db::Db;
use crate::socket::Slash;
use crate::worker::Job;

fn slash(envelope_id: &str, user: &str, channel: &str, text: &str) -> Slash {
    Slash {
        envelope_id: envelope_id.into(),
        channel: channel.into(),
        user: user.into(),
        text: text.into(),
        trigger_id: None,
        response_url: None,
    }
}

/// Records `s` with channel `C1` allowed; returns the ack text and the job.
fn run(db: &Db, s: &Slash) -> (String, Option<Job>) {
    let channels = vec!["C1".to_owned()];
    let (kept, job) = record(s, db, &channels, 1_000).unwrap();
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
    assert_eq!(job, None);
    let rows = db.channel_repos("C1").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].repo.as_str(), rows[0].added_by.as_str()),
        ("acme/api", "U1")
    );
}

#[test]
fn a_repo_that_is_not_owner_name_is_refused_and_not_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    for (n, repo) in ["api", "a/b/c", "..", "a/.."].into_iter().enumerate() {
        let (text, _) = run(
            &db,
            &slash(&format!("e{n}"), "U1", "C1", &format!("repo add {repo}")),
        );
        assert_eq!(
            text,
            format!("`{repo}` is not a GitHub repo name. Use `/moor repo add <owner/name>`.")
        );
    }
    assert!(db.channel_repos("C1").unwrap().is_empty());
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
fn a_channel_off_the_list_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (text, job) = run(&db, &slash("e2", "U1", "C9", "repo add acme/api"));
    assert_eq!(text, "moor does not work in this channel.");
    assert_eq!(job, None);
    assert!(db.channel_repos("C9").unwrap().is_empty());
}

#[test]
fn anyone_in_an_allowed_channel_changes_its_repos() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let (text, _) = run(&db, &slash("e1", "U9", "C1", "repo add acme/api"));
    assert_eq!(text, "Recorded. This channel works on `acme/api`.");
    assert_eq!(db.channel_repos("C1").unwrap().len(), 1);
}
