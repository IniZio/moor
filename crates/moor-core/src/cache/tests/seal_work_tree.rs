//! The work tree's seal run under a real `sh` against a real git work tree at `~/moor/repos/r`.

use super::*;
use crate::testgit::{Origin, BRANCH};

/// A repo Box's plan for repo `o/r` with every preset off, so only the work tree seals.
fn tree_plan() -> CachePlan {
    plan(&layers("cache: {presets: false}", ""))
}

/// A work tree cloned from `o`, on [`BRANCH`] pushed to the origin at the base, with an ignored
/// `build/out`.
fn cloned(name: &str) -> Origin {
    let o = Origin::new(name);
    let tree = o.tree();
    o.git(
        &o.home(),
        &[
            "clone",
            "-q",
            "--filter=blob:none",
            &o.url(),
            "moor/repos/r",
        ],
    );
    o.git(&tree, &["checkout", "-q", "-b", BRANCH]);
    o.git(&tree, &["push", "-q", "origin", BRANCH]);
    std::fs::create_dir_all(tree.join("build")).unwrap();
    std::fs::write(tree.join("build/out"), "artifact").unwrap();
    o
}

/// Runs the plan's seal for base `base`; returns stdout, after asserting a zero exit.
fn seal(o: &Origin, base: Option<&str>) -> String {
    let script = tree_plan().seal_script(base).unwrap();
    let out = o.sh(&o.home(), &script, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn files(o: &Origin) -> Vec<String> {
    Origin::entries(&o.tree())
}

fn marked(o: &Origin) -> bool {
    o.tree().join(".crabbox-sealed").exists()
}

#[test]
fn a_clean_tree_and_a_dirty_but_pushed_tree_are_marked() {
    let o = cloned("clean");
    let out = seal(&o, Some(&o.base));
    assert!(
        not_kept(&out).is_empty() && seal_failed(&out).is_empty(),
        "{out}"
    );
    assert!(marked(&o));
    std::fs::write(o.tree().join("a.txt"), "modified\n").unwrap();
    std::fs::write(o.tree().join("untracked"), "x").unwrap();
    let out = seal(&o, Some(&o.base));
    assert!(not_kept(&out).is_empty(), "{out}");
    assert!(
        marked(&o),
        "modified and untracked files do not block keeping"
    );
}

/// Seals a tree `spoil` left holding what no remote has; asserts it is named with `why`, not
/// marked and not emptied.
fn refused(name: &str, spoil: fn(&Origin), why: &str) {
    let o = cloned(name);
    std::fs::write(o.tree().join(".crabbox-sealed"), "old").unwrap();
    spoil(&o);
    let mut left = files(&o);
    left.retain(|f| f != ".crabbox-sealed");
    let out = seal(&o, Some(&o.base));
    let [got]: [NotKept; 1] = not_kept(&out).try_into().unwrap();
    assert_eq!(got.name, "worktree", "{name}");
    assert!(got.reason.contains(why), "{name}: {}", got.reason);
    assert!(
        seal_failed(&out).is_empty(),
        "{name}: the tree is not emptied"
    );
    assert!(!marked(&o), "{name}: no marker");
    assert_eq!(files(&o), left, "{name}: nothing deleted");
    let out_file = std::fs::read_to_string(o.tree().join("build/out")).unwrap();
    assert_eq!(out_file, "artifact", "{name}");
}

fn commit_a(o: &Origin, text: &str) {
    std::fs::write(o.tree().join("a.txt"), text).unwrap();
    o.git(&o.tree(), &["commit", "-q", "-am", text]);
}

#[test]
fn an_unpushed_commit_on_the_branch_another_branch_or_a_detached_head_is_not_kept() {
    refused("branch", |o| commit_a(o, "local\n"), "1 unpushed commits");
    refused(
        "side",
        |o| {
            o.git(&o.tree(), &["checkout", "-q", "-b", "side"]);
            commit_a(o, "side\n");
            o.git(&o.tree(), &["checkout", "-q", BRANCH]);
        },
        "1 unpushed commits",
    );
    refused(
        "detached",
        |o| {
            o.git(&o.tree(), &["checkout", "-q", "--detach"]);
            commit_a(o, "detached\n");
        },
        "1 unpushed commits",
    );
}

#[test]
fn a_stash_a_staged_change_or_a_rebase_in_progress_is_not_kept() {
    refused(
        "stash",
        |o| {
            std::fs::write(o.tree().join("a.txt"), "stashed\n").unwrap();
            o.git(&o.tree(), &["stash", "-q"]);
        },
        "stash",
    );
    refused(
        "staged",
        |o| {
            std::fs::write(o.tree().join("a.txt"), "staged\n").unwrap();
            o.git(&o.tree(), &["add", "a.txt"]);
        },
        "staged",
    );
    refused(
        "rebase",
        |o| {
            o.push("moor/conflict", &[("a.txt", "remote\n")]);
            o.git(&o.tree(), &["fetch", "-q", "origin"]);
            commit_a(o, "local\n");
            o.git(&o.tree(), &["push", "-q", "origin", BRANCH]);
            let out = o.try_git(&o.tree(), &["rebase", "origin/moor/conflict"]);
            assert!(!out.status.success(), "{out:?}");
        },
        "in progress",
    );
}

#[test]
fn a_base_no_remote_branch_holds_counts_as_pushed() {
    let o = cloned("base");
    let pr = o.push("refs-only", &[("p.txt", "p\n")]);
    o.git(
        &o.seed(),
        &["push", "-q", "origin", ":refs/heads/refs-only"],
    );
    o.git(&o.tree(), &["fetch", "-q", "origin", &pr]);
    o.git(&o.tree(), &["checkout", "-q", "-B", BRANCH, &pr]);
    let out = seal(&o, None);
    assert_eq!(
        not_kept(&out).len(),
        1,
        "without the base the commit is unpushed: {out}"
    );
    let out = seal(&o, Some(&pr));
    assert!(not_kept(&out).is_empty(), "{out}");
    assert!(marked(&o));
}

#[test]
fn a_missing_work_tree_is_not_kept_and_says_why() {
    let o = Origin::new("missing");
    std::fs::remove_dir_all(o.tree()).unwrap();
    let out = seal(&o, None);
    let [got]: [NotKept; 1] = not_kept(&out).try_into().unwrap();
    assert!(got.reason.contains("not a git checkout"), "{}", got.reason);
}
