//! The refresh, followed by the clone script, run under `sh` with real git against a local bare
//! origin, in a work tree a previous Box left.

use std::path::Path;
use std::process::Output;

use super::refresh_script;
use crate::boxscript::clone::{clone_script, Checkout};
use crate::testgit::{Origin, BRANCH};

const BOX1: &str = "moor-1-aaaaaa";
const BOX2: &str = "moor-2-bbbbbb";

/// Runs the refresh and the clone for `branch` at `base` as Box `key`, in the work tree.
fn start(o: &Origin, key: &str, base: &str, branch: &str) -> Output {
    let url = o.url();
    let c = Checkout {
        url: &url,
        base,
        branch,
    };
    let script = format!("{}{}", refresh_script(&c, key), clone_script(&c));
    o.sh(&o.tree(), &script, &[])
}

/// As [`start`] on [`BRANCH`]; asserts success and returns stdout.
fn ok(o: &Origin, key: &str, base: &str) -> String {
    ok_on(o, key, base, BRANCH)
}

fn ok_on(o: &Origin, key: &str, base: &str, branch: &str) -> String {
    let out = start(o, key, base, branch);
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn tree_git(o: &Origin, args: &[&str]) -> String {
    o.git(&o.tree(), args)
}

fn read(o: &Origin, rel: &str) -> String {
    std::fs::read_to_string(o.tree().join(rel)).unwrap()
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

fn local_branches(o: &Origin) -> String {
    tree_git(o, &["for-each-ref", "--format=%(refname)", "refs/heads"])
}

fn clean_on(o: &Origin, head: &str) {
    assert_eq!(tree_git(o, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(tree_git(o, &["rev-parse", "HEAD"]), head);
    assert_eq!(tree_git(o, &["status", "--porcelain", "--ignored=no"]), "");
}

#[test]
fn an_empty_tree_is_cloned_cold_and_marked_with_the_box() {
    let o = Origin::new("cold");
    let out = ok(&o, BOX1, &o.base);
    assert!(out.contains("moor: work tree cold\n"), "{out}");
    clean_on(&o, &o.base);
    assert_eq!(read(&o, ".moor/tree"), format!("{BOX1}\n"));
}

#[test]
fn the_same_branch_moves_to_its_remote_tip_and_keeps_outputs_and_mtimes() {
    let o = Origin::new("same");
    ok(&o, BOX1, &o.base);
    std::fs::create_dir_all(o.tree().join("build")).unwrap();
    std::fs::write(o.tree().join("build/out"), "artifact").unwrap();
    let old = o.sh(&o.tree(), "touch -d @1000000000 b.txt", &[]);
    assert!(old.status.success(), "{old:?}");
    let before = mtime(&o.tree().join("b.txt"));
    let tip = o.push(BRANCH, &[("a.txt", "two\n")]);
    let out = ok(&o, BOX2, &o.base);
    assert!(out.contains("moor: work tree refreshed\n"), "{out}");
    clean_on(&o, &tip);
    assert_eq!(read(&o, "a.txt"), "two\n", "the changed file is rewritten");
    assert_eq!(
        mtime(&o.tree().join("b.txt")),
        before,
        "an unchanged file keeps its mtime"
    );
    assert_eq!(
        read(&o, "build/out"),
        "artifact",
        "an ignored output survives"
    );
    assert_eq!(read(&o, ".moor/tree"), format!("{BOX2}\n"));
}

#[test]
fn a_different_branch_is_checked_out_from_the_remote() {
    let o = Origin::new("other");
    ok(&o, BOX1, &o.base);
    let tip = o.push("moor/other", &[("c.txt", "c\n")]);
    ok_on(&o, BOX2, &o.base, "moor/other");
    assert_eq!(tree_git(&o, &["rev-parse", "HEAD"]), tip);
    assert_eq!(read(&o, "c.txt"), "c\n");
    assert_eq!(local_branches(&o), "refs/heads/moor/other");
}

#[test]
fn a_branch_deleted_on_the_remote_starts_from_the_base() {
    let o = Origin::new("gone");
    let tip = o.push(BRANCH, &[("a.txt", "two\n")]);
    ok(&o, BOX1, &o.base);
    assert_eq!(tree_git(&o, &["rev-parse", "HEAD"]), tip);
    o.git(
        &o.seed(),
        &["push", "-q", "origin", &format!(":refs/heads/{BRANCH}")],
    );
    ok(&o, BOX2, &o.base);
    clean_on(&o, &o.base);
    assert_eq!(read(&o, "a.txt"), "one\n");
}

#[test]
fn a_base_of_head_is_the_remote_default_branch_not_the_old_local_one() {
    let o = Origin::new("head");
    ok(&o, BOX1, &o.base);
    let main = o.push("main", &[("a.txt", "main two\n")]);
    ok_on(&o, BOX2, "HEAD", "moor/fresh");
    assert_eq!(tree_git(&o, &["rev-parse", "HEAD"]), main);
    assert_eq!(read(&o, "a.txt"), "main two\n");
}

#[test]
fn a_partial_clone_fetches_only_the_blobs_its_checkout_needs() {
    let o = Origin::new("partial");
    ok(&o, BOX1, &o.base);
    o.push("moor/unrelated", &[("z.txt", "never checked out\n")]);
    let unrelated = o.git(&o.seed(), &["rev-parse", "HEAD:z.txt"]);
    o.git(&o.seed(), &["reset", "-q", "--hard", &o.base]);
    o.push(BRANCH, &[("big.txt", "fetched lazily\n")]);
    ok(&o, BOX2, &o.base);
    assert_eq!(read(&o, "big.txt"), "fetched lazily\n");
    let filter = tree_git(&o, &["config", "remote.origin.partialclonefilter"]);
    assert_eq!(filter, "blob:none");
    let probe = o.sh(
        &o.tree(),
        &format!("GIT_NO_LAZY_FETCH=1 git cat-file -e {unrelated}"),
        &[],
    );
    assert!(
        !probe.status.success(),
        "a blob no checkout needed was fetched"
    );
}

/// A Box 1 tree stopped mid-`op` on a conflict with `moor/conflict`.
fn stopped_mid(op: &str) -> Origin {
    let o = Origin::new(op);
    o.push("moor/conflict", &[("a.txt", "remote\n")]);
    ok(&o, BOX1, &o.base);
    std::fs::write(o.tree().join("a.txt"), "local\n").unwrap();
    tree_git(&o, &["commit", "-q", "-am", "local"]);
    let args: &[&str] = match op {
        "rebase" => &["rebase", "origin/moor/conflict"],
        "merge" => &["merge", "origin/moor/conflict"],
        _ => &["cherry-pick", "origin/moor/conflict"],
    };
    let out = o.try_git(&o.tree(), args);
    assert!(!out.status.success(), "{op} stops on its conflict: {out:?}");
    o
}

#[test]
fn a_rebase_merge_or_cherry_pick_in_progress_ends_clean() {
    for (op, left) in [
        ("rebase", ".git/rebase-merge"),
        ("merge", ".git/MERGE_HEAD"),
        ("cherry-pick", ".git/CHERRY_PICK_HEAD"),
    ] {
        let o = stopped_mid(op);
        assert!(o.tree().join(left).exists(), "{op}");
        let out = ok(&o, BOX2, &o.base);
        assert!(out.contains("moor: work tree refreshed\n"), "{op}: {out}");
        assert!(!o.tree().join(left).exists(), "{op}");
        clean_on(&o, &o.base);
        assert_eq!(read(&o, "a.txt"), "one\n", "{op}");
    }
}

#[test]
fn an_initialized_submodule_follows_the_new_gitlink() {
    let o = Origin::new("sub");
    let sub = o.path("subseed");
    o.git(&o.path(""), &["init", "-q", "-b", "main", "subseed"]);
    std::fs::write(sub.join("s.txt"), "s1\n").unwrap();
    o.git(&sub, &["add", "."]);
    o.git(&sub, &["commit", "-q", "-m", "s1"]);
    let s1 = o.git(&sub, &["rev-parse", "HEAD"]);
    let sub_url = format!("file://{}", sub.display());
    o.git(&o.seed(), &["submodule", "add", "-q", &sub_url, "lib"]);
    o.push(BRANCH, &[]);
    ok(&o, BOX1, &o.base);
    tree_git(&o, &["submodule", "update", "-q", "--init"]);
    assert_eq!(o.git(&o.tree().join("lib"), &["rev-parse", "HEAD"]), s1);
    std::fs::write(o.tree().join("lib/junk"), "x").unwrap();
    std::fs::write(sub.join("s.txt"), "s2\n").unwrap();
    o.git(&sub, &["commit", "-q", "-am", "s2"]);
    let s2 = o.git(&sub, &["rev-parse", "HEAD"]);
    o.git(&o.seed().join("lib"), &["pull", "-q", "origin", "main"]);
    let tip = o.push(BRANCH, &[]);
    ok(&o, BOX2, &o.base);
    clean_on(&o, &tip);
    assert_eq!(o.git(&o.tree().join("lib"), &["rev-parse", "HEAD"]), s2);
    assert_eq!(read(&o, "lib/s.txt"), "s2\n");
    assert!(
        !o.tree().join("lib/junk").exists(),
        "the submodule is cleaned"
    );
}

#[test]
fn the_previous_sessions_files_refs_stash_and_reflog_are_gone() {
    let o = Origin::new("leftovers");
    ok(&o, BOX1, &o.base);
    let t = o.tree();
    std::fs::write(t.join("u.txt"), "untracked").unwrap();
    std::fs::create_dir_all(t.join("nested")).unwrap();
    o.git(&t.join("nested"), &["init", "-q"]);
    std::fs::write(t.join("nested/n"), "n").unwrap();
    std::fs::write(t.join(".moor/brief"), "old brief").unwrap();
    std::fs::create_dir_all(t.join(".crabbox")).unwrap();
    std::fs::write(t.join(".crabbox/lease"), "old").unwrap();
    std::fs::write(t.join(".crabbox-sealed"), "").unwrap();
    tree_git(&o, &["branch", "scratch"]);
    std::fs::write(t.join("a.txt"), "stashed\n").unwrap();
    tree_git(&o, &["stash", "-q"]);
    std::fs::write(t.join("b.txt"), "dirty\n").unwrap();
    ok(&o, BOX2, &o.base);
    clean_on(&o, &o.base);
    assert_eq!(read(&o, "b.txt"), "unchanged\n");
    for gone in [
        "u.txt",
        "nested",
        ".crabbox",
        ".crabbox-sealed",
        ".moor/brief",
    ] {
        assert!(!t.join(gone).exists(), "{gone} is left");
    }
    assert_eq!(Origin::entries(&t.join(".moor")), ["tree"]);
    assert_eq!(local_branches(&o), format!("refs/heads/{BRANCH}"));
    let stash = o.try_git(&t, &["rev-parse", "-q", "--verify", "refs/stash"]);
    assert!(!stash.status.success(), "the stash is gone");
    assert_eq!(tree_git(&o, &["reflog", "show", "--all"]), "");
    tree_git(&o, &["check-ignore", "-q", ".crabbox-sealed"]);
}

#[test]
fn a_corrupt_git_dir_or_another_origin_empties_the_tree_and_clones_cold() {
    let corrupt: fn(&Origin) = |o| std::fs::write(o.tree().join(".git/HEAD"), "garbage").unwrap();
    let elsewhere: fn(&Origin) = |o| {
        tree_git(o, &["remote", "set-url", "origin", "file:///elsewhere.git"]);
    };
    for (name, spoil) in [("corrupt", corrupt), ("mismatch", elsewhere)] {
        let o = Origin::new(name);
        ok(&o, BOX1, &o.base);
        std::fs::write(o.tree().join("stale.txt"), "x").unwrap();
        spoil(&o);
        let out = ok(&o, BOX2, &o.base);
        assert!(
            out.contains("moor: work tree cold after: "),
            "{name}: {out}"
        );
        assert!(!o.tree().join("stale.txt").exists(), "{name}");
        clean_on(&o, &o.base);
        assert_eq!(tree_git(&o, &["remote", "get-url", "origin"]), o.url());
    }
}

#[test]
fn a_rerun_on_the_same_box_keeps_a_commit_made_after_the_first_run() {
    let o = Origin::new("rerun");
    ok(&o, BOX1, &o.base);
    std::fs::write(o.tree().join("a.txt"), "agent\n").unwrap();
    tree_git(&o, &["commit", "-q", "-am", "agent"]);
    let agent = tree_git(&o, &["rev-parse", "HEAD"]);
    let out = ok(&o, BOX1, &o.base);
    assert!(!out.contains("moor: work tree"), "{out}");
    assert_eq!(tree_git(&o, &["rev-parse", "HEAD"]), agent);
}

#[test]
fn refs_outside_the_box_branch_origin_and_tags_and_other_remotes_are_gone() {
    let o = Origin::new("refs");
    ok(&o, BOX1, &o.base);
    let t = o.tree();
    o.git(&t, &["checkout", "-q", "--detach"]);
    std::fs::write(t.join("a.txt"), "hidden\n").unwrap();
    o.git(&t, &["commit", "-q", "-am", "hidden"]);
    let hidden = o.git(&t, &["rev-parse", "HEAD"]);
    o.git(&t, &["checkout", "-q", BRANCH]);
    o.git(&t, &["update-ref", "refs/wip/x", &hidden]);
    o.git(&t, &["remote", "add", "other", "file:///nowhere.git"]);
    o.git(&t, &["update-ref", "refs/remotes/other/x", &hidden]);
    o.git(&t, &["notes", "add", "-m", "note", "HEAD"]);
    o.git(&t, &["tag", "kept-tag", &o.base]);
    o.git(&o.seed(), &["tag", "kept-tag", &o.base]);
    o.git(&o.seed(), &["push", "-q", "origin", "kept-tag"]);
    ok(&o, BOX2, &o.base);
    let refs = tree_git(&o, &["for-each-ref", "--format=%(refname)"]);
    let left: Vec<&str> = refs
        .lines()
        .filter(|r| !r.starts_with("refs/remotes/origin/"))
        .collect();
    assert_eq!(
        left,
        [
            format!("refs/heads/{BRANCH}").as_str(),
            "refs/tags/kept-tag"
        ]
    );
    assert_eq!(tree_git(&o, &["remote"]), "origin");
    let probe = o.try_git(&t, &["cat-file", "-e", &format!("{hidden}^{{commit}}")]);
    let reachable = tree_git(&o, &["branch", "-a", "--contains", &hidden]);
    assert!(
        probe.status.success() && reachable.is_empty(),
        "no ref reaches it"
    );
}

#[test]
fn a_bisect_or_a_cherry_pick_sequence_in_progress_ends() {
    let o = Origin::new("bisect");
    o.push(BRANCH, &[("a.txt", "two\n")]);
    o.push(BRANCH, &[("a.txt", "three\n")]);
    ok(&o, BOX1, &o.base);
    let t = o.tree();
    o.git(&t, &["bisect", "start", "HEAD", &o.base]);
    assert!(t.join(".git/BISECT_START").exists());
    ok(&o, BOX2, &o.base);
    assert!(!t.join(".git/BISECT_START").exists(), "the bisect ended");
    assert!(!o.try_git(&t, &["bisect", "log"]).status.success());

    let o = Origin::new("sequence");
    o.push("moor/conflict", &[("a.txt", "c1\n")]);
    o.push("moor/conflict", &[("a.txt", "c2\n")]);
    ok(&o, BOX1, &o.base);
    let t = o.tree();
    std::fs::write(t.join("a.txt"), "local\n").unwrap();
    o.git(&t, &["commit", "-q", "-am", "local"]);
    let range = format!("{}..origin/moor/conflict", o.base);
    let out = o.try_git(&t, &["cherry-pick", &range]);
    assert!(!out.status.success(), "{out:?}");
    assert!(t.join(".git/sequencer").exists());
    ok(&o, BOX2, &o.base);
    assert!(!t.join(".git/sequencer").exists(), "the sequence ended");
    clean_on(&o, &o.base);
}

/// Every ref and every pseudo-ref file in the tree's `.git` that `commit` is reachable from.
fn names_reaching(o: &Origin, commit: &str) -> Vec<String> {
    let refs = tree_git(o, &["for-each-ref", "--format=%(refname)"]);
    let pseudo = Origin::entries(&o.tree().join(".git"))
        .into_iter()
        .filter(|n| n.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'));
    refs.lines()
        .map(str::to_owned)
        .chain(pseudo)
        .filter(|n| {
            let probe = o.try_git(&o.tree(), &["merge-base", "--is-ancestor", commit, n]);
            probe.status.success()
        })
        .collect()
}

/// Box 1 commits in the tree and returns the commit.
fn unpushed_commit(o: &Origin) -> String {
    ok(o, BOX1, &o.base);
    std::fs::write(o.tree().join("a.txt"), "unpushed\n").unwrap();
    tree_git(o, &["commit", "-q", "-am", "unpushed"]);
    tree_git(o, &["rev-parse", "HEAD"])
}

#[test]
fn a_commit_the_previous_session_reset_away_is_reachable_by_no_name() {
    for left in [
        "ORIG_HEAD",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "AUTO_MERGE",
        "BISECT_EXPECTED_REV",
    ] {
        let o = Origin::new(left);
        let gone = unpushed_commit(&o);
        tree_git(&o, &["reset", "-q", "--hard", "HEAD~1"]);
        std::fs::write(o.tree().join(".git").join(left), format!("{gone}\n")).unwrap();
        assert!(
            names_reaching(&o, &gone).contains(&left.to_owned()),
            "{left}"
        );
        let out = ok(&o, BOX2, &o.base);
        assert!(out.contains("moor: work tree refreshed\n"), "{left}: {out}");
        assert_eq!(names_reaching(&o, &gone), Vec::<String>::new(), "{left}");
    }
}

#[test]
fn a_local_tag_on_an_unpushed_commit_is_gone() {
    let o = Origin::new("tag");
    let tagged = unpushed_commit(&o);
    tree_git(&o, &["tag", "wip", &tagged]);
    let out = ok(&o, BOX2, &o.base);
    assert!(out.contains("moor: work tree refreshed\n"), "{out}");
    assert_eq!(names_reaching(&o, &tagged), Vec::<String>::new());
}

#[test]
fn the_outcome_is_the_text_after_the_log_prefix() {
    for (text, want) in [
        ("moor: work tree refreshed\n", Some("refreshed")),
        ("moor: work tree cold\n", Some("cold")),
        (
            "user.name=0\nmoor: work tree cold after: git fetch failed\nmoor.cpus=2\n",
            Some("cold after: git fetch failed"),
        ),
        ("Cloning into '.'\n", None),
        ("", None),
    ] {
        assert_eq!(super::outcome(text), want, "{text:?}");
    }
}

#[test]
fn the_outcome_names_what_the_script_printed() {
    let o = Origin::new("outcome");
    let first = ok(&o, BOX1, &o.base);
    assert_eq!(super::outcome(&first), Some("cold"), "{first}");
    let second = ok(&o, BOX2, &o.base);
    assert_eq!(super::outcome(&second), Some("refreshed"), "{second}");
    let rerun = ok(&o, BOX2, &o.base);
    assert_eq!(super::outcome(&rerun), None, "{rerun}");
}
