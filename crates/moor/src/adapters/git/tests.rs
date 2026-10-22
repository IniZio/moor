//! Git adapter tests over real git in a temp checkout.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use moor_core::seam::checkout::Checkout;

use super::Git;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;

fn real_git() -> Git {
    let env = Env::from_pairs([
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("HOME", std::env::var("HOME").unwrap_or_default()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".to_owned()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_owned()),
    ]);
    Git::new(Arc::new(SystemProc::new(&env, &ToolPaths::default())))
}

fn git_in(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

/// A checkout with one commit pushed to a local bare `origin`, tracking `origin/main`.
fn pushed(root: &Path) -> std::path::PathBuf {
    let bare = root.join("origin.git");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git_in(root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git_in(&work, &["init", "-q", "-b", "main"]);
    git_in(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git_in(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&work, &["push", "-q", "-u", "origin", "main"]);
    work
}

#[test]
fn origin_url_reads_the_origin_remote() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("w");
    std::fs::create_dir_all(&work).unwrap();
    git_in(&work, &["init", "-q"]);
    assert_eq!(real_git().origin_url(&work), None);
    git_in(
        &work,
        &["remote", "add", "origin", "git@github.com:o/n.git"],
    );
    assert_eq!(
        real_git().origin_url(&work).as_deref(),
        Some("git@github.com:o/n.git")
    );
}

#[test]
fn origin_url_outside_a_checkout_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(real_git().origin_url(dir.path()), None);
}

#[test]
fn local_changes_sees_edits_and_unpushed_commits_only() {
    let dir = tempfile::tempdir().unwrap();
    let work = pushed(dir.path());
    let git = real_git();
    assert!(!git.local_changes(&work), "clean and pushed");
    std::fs::write(work.join("new.txt"), "x").unwrap();
    assert!(git.local_changes(&work), "untracked file");
    std::fs::remove_file(work.join("new.txt")).unwrap();
    git_in(&work, &["commit", "-q", "--allow-empty", "-m", "two"]);
    assert!(git.local_changes(&work), "ahead of origin/main");
    assert!(!git.local_changes(dir.path()), "not a checkout");
}

#[test]
fn a_branch_with_no_upstream_counts_as_local_changes() {
    let dir = tempfile::tempdir().unwrap();
    let work = pushed(dir.path());
    let git = real_git();
    git_in(&work, &["switch", "-q", "-c", "topic"]);
    assert!(git.local_changes(&work), "no upstream");
    git_in(&work, &["switch", "-q", "--detach", "main"]);
    assert!(git.local_changes(&work), "detached HEAD");
}
