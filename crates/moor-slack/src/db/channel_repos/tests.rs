//! Tests for each channel's repos and its one default.

use rusqlite::params;

use super::{ChannelRepo, Db};

fn open(dir: &std::path::Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

/// `(repo, is_default)` of each repo of `channel`.
fn listed(db: &Db, channel: &str) -> Vec<(String, bool)> {
    db.channel_repos(channel)
        .unwrap()
        .into_iter()
        .map(|r| (r.repo, r.is_default))
        .collect()
}

fn owned(rows: &[(&str, bool)]) -> Vec<(String, bool)> {
    rows.iter().map(|(r, d)| ((*r).to_owned(), *d)).collect()
}

#[test]
fn a_channel_keeps_several_repos_with_who_added_each() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    assert!(db.add_channel_repo("C1", "acme/web", "U1", 1).unwrap());
    assert!(db.add_channel_repo("C1", "acme/api", "U2", 2).unwrap());
    assert!(!db.add_channel_repo("C1", "acme/api", "U3", 3).unwrap());
    assert!(db.add_channel_repo("C2", "acme/api", "U3", 4).unwrap());
    let want = vec![
        ChannelRepo {
            repo: "acme/api".into(),
            added_by: "U2".into(),
            is_default: false,
        },
        ChannelRepo {
            repo: "acme/web".into(),
            added_by: "U1".into(),
            is_default: false,
        },
    ];
    assert_eq!(db.channel_repos("C1").unwrap(), want);
    assert_eq!(listed(&db, "C2"), owned(&[("acme/api", false)]));
}

#[test]
fn a_second_default_replaces_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.add_channel_repo("C1", "acme/api", "U1", 1).unwrap();
    db.add_channel_repo("C1", "acme/web", "U1", 2).unwrap();
    db.add_channel_repo("C2", "acme/api", "U1", 3).unwrap();
    assert!(db
        .atomically(|| db.set_default_channel_repo("C2", "acme/api"))
        .unwrap());
    assert!(db
        .atomically(|| db.set_default_channel_repo("C1", "acme/api"))
        .unwrap());
    assert!(db
        .atomically(|| db.set_default_channel_repo("C1", "acme/web"))
        .unwrap());
    assert_eq!(
        listed(&db, "C1"),
        owned(&[("acme/api", false), ("acme/web", true)])
    );
    assert_eq!(listed(&db, "C2"), owned(&[("acme/api", true)]));
}

#[test]
fn a_repo_the_channel_lacks_cannot_become_its_default() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.add_channel_repo("C1", "acme/api", "U1", 1).unwrap();
    db.atomically(|| db.set_default_channel_repo("C1", "acme/api"))
        .unwrap();
    assert!(!db
        .atomically(|| db.set_default_channel_repo("C1", "acme/web"))
        .unwrap());
    assert_eq!(listed(&db, "C1"), owned(&[("acme/api", true)]));
}

#[test]
fn removing_the_default_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    db.add_channel_repo("C1", "acme/api", "U1", 1).unwrap();
    db.add_channel_repo("C1", "acme/web", "U1", 2).unwrap();
    db.atomically(|| db.set_default_channel_repo("C1", "acme/api"))
        .unwrap();
    assert!(db.remove_channel_repo("C1", "acme/api").unwrap());
    assert!(!db.remove_channel_repo("C1", "acme/api").unwrap());
    assert_eq!(listed(&db, "C1"), owned(&[("acme/web", false)]));
    db.add_channel_repo("C1", "acme/api", "U1", 3).unwrap();
    assert_eq!(
        listed(&db, "C1"),
        owned(&[("acme/api", false), ("acme/web", false)])
    );
}

#[test]
fn the_schema_refuses_a_second_default_in_one_channel() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(dir.path());
    let insert = |channel: &str, repo: &str| {
        db.conn.execute(
            "INSERT INTO slack_channel_repos (channel, repo, added_by, added_at, is_default) \
             VALUES (?1, ?2, 'U1', 1, 1)",
            params![channel, repo],
        )
    };
    insert("C1", "acme/api").unwrap();
    insert("C2", "acme/api").unwrap();
    assert!(insert("C1", "acme/web").is_err());
}
