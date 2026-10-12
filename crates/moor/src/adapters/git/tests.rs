//! Git adapter tests: scripted replies plus one real-git scenario.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use moor_core::seam::proc::Tool;

use super::{Git, GitError};
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;
use crate::testing::proc::{Reply, ScriptedProc};

fn scripted() -> (Arc<ScriptedProc>, Git) {
    let p = Arc::new(ScriptedProc::new());
    (Arc::clone(&p), Git::new(p))
}

fn ok(s: &str) -> Reply {
    Reply::Ok(s.to_owned())
}

#[test]
fn toplevel_trims_output_and_runs_in_cwd() {
    let (p, g) = scripted();
    p.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        ok("/r/repo\n"),
    );
    let root = g.toplevel(Path::new("/r/repo/sub")).unwrap();
    assert_eq!(root, Path::new("/r/repo"));
    let calls = p.calls();
    let first = calls.first().map(|c| c.cwd.as_deref());
    assert_eq!(first, Some(Some(Path::new("/r/repo/sub"))));
}

#[test]
fn toplevel_outside_repo_is_not_a_repo() {
    let (p, g) = scripted();
    p.on(
        Tool::Git,
        &["rev-parse"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "fatal: not a git repository (or any parent)".to_owned(),
        },
    );
    assert!(matches!(
        g.toplevel(Path::new("/x")),
        Err(GitError::NotARepo)
    ));
}

#[test]
fn other_failure_is_moor_worded() {
    let (p, g) = scripted();
    p.on(
        Tool::Git,
        &["rev-parse"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "boom\n".to_owned(),
        },
    );
    assert!(matches!(
        g.toplevel(Path::new("/x")),
        Err(GitError::Failed(m)) if m == "git rev-parse failed (exit 1): boom"
    ));
}

#[test]
fn timeout_is_failed() {
    let (p, g) = scripted();
    p.on(Tool::Git, &["rev-parse"], Reply::Timeout);
    assert!(matches!(
        g.toplevel(Path::new("/x")),
        Err(GitError::Failed(_))
    ));
}

#[test]
fn unpushed_counts_commits_not_on_any_remote() {
    let dir = tempfile::tempdir().unwrap();
    let (p, g) = scripted();
    p.on(
        Tool::Git,
        &["rev-list", "--count", "HEAD", "--not", "--remotes"],
        ok("3\n"),
    );
    assert_eq!(g.unpushed(dir.path(), None).unwrap(), 3);
}

#[test]
fn unpushed_rejects_garbage_count() {
    let dir = tempfile::tempdir().unwrap();
    let (p, g) = scripted();
    p.on(Tool::Git, &["rev-list"], ok("many\n"));
    assert!(matches!(
        g.unpushed(dir.path(), None),
        Err(GitError::Failed(_))
    ));
}

#[test]
fn dirty_reads_porcelain() {
    let dir = tempfile::tempdir().unwrap();
    let (p, g) = scripted();
    p.on(Tool::Git, &["status", "--porcelain"], ok(" M a.txt\n"));
    assert!(g.dirty(dir.path()).unwrap());
}

#[test]
fn clean_porcelain_is_not_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let (p, g) = scripted();
    p.on(Tool::Git, &["status", "--porcelain"], ok("\n"));
    assert!(!g.dirty(dir.path()).unwrap());
}

#[test]
fn missing_worktree_is_zero_and_clean_without_running_git() {
    let (p, g) = scripted();
    let gone = Path::new("/nonexistent/moor/worktree");
    assert_eq!(g.unpushed(gone, None).unwrap(), 0);
    assert!(!g.dirty(gone).unwrap());
    assert!(p.calls().is_empty());
}

fn real_git() -> Git {
    let env = Env::from_pairs([
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("HOME", std::env::var("HOME").unwrap_or_default()),
    ]);
    Git::new(Arc::new(SystemProc::new(&env, &ToolPaths::default())))
}

fn git_in(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn real_git_unpushed_without_upstream_and_dirty() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git_in(d, &["init", "-q"]);
    std::fs::write(d.join("a.txt"), "a").unwrap();
    git_in(d, &["add", "a.txt"]);
    git_in(d, &["commit", "-q", "-m", "one"]);
    let g = real_git();
    assert_eq!(g.unpushed(d, None).unwrap(), 1);
    assert!(!g.dirty(d).unwrap());
    std::fs::write(d.join("b.txt"), "b").unwrap();
    assert!(g.dirty(d).unwrap());
    let root = g.toplevel(d).unwrap();
    assert_eq!(root.canonicalize().unwrap(), d.canonicalize().unwrap());
    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(g.toplevel(plain.path()), Err(GitError::NotARepo)));
}

#[test]
fn real_git_unpushed_excludes_base() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git_in(d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "a").unwrap();
    git_in(d, &["add", "a.txt"]);
    git_in(d, &["commit", "-q", "-m", "base"]);
    git_in(d, &["checkout", "-q", "-b", "task"]);
    std::fs::write(d.join("b.txt"), "b").unwrap();
    git_in(d, &["add", "b.txt"]);
    git_in(d, &["commit", "-q", "-m", "work"]);
    let g = real_git();
    assert_eq!(g.unpushed(d, Some("main")).unwrap(), 1);
    assert_eq!(g.unpushed(d, None).unwrap(), 2);
}
