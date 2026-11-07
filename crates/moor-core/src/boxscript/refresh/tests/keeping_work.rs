//! The refresh of a fork at its own commit: it learns what was pushed and drops the source Box's
//! refs, and keeps what skipped setup lines made.

use std::process::Output;

use super::super::{outcome, refresh_with, went_cold, Work};
use super::{ok, read, tree_git, BOX1, BOX2};
use crate::boxscript::clone::{clone_script, Checkout};
use crate::testgit::{Origin, BRANCH};

/// Runs the refresh and the clone on `branch` at `base` as `key`, in the tree.
fn start_keeping(o: &Origin, key: &str, branch: &str) -> Output {
    let url = o.url();
    let c = Checkout {
        url: &url,
        base: &o.base,
        branch,
    };
    let script = format!("{}{}", refresh_with(&c, key, Work::Keep), clone_script(&c));
    let out = o.sh(&o.tree(), &script, &[]);
    assert!(out.status.success(), "{out:?}");
    out
}

/// A tree a first Box left at the base, with one tracked change and one untracked file.
fn left_with_work() -> Origin {
    let o = Origin::new("keeping");
    ok(&o, BOX1, &o.base);
    std::fs::write(o.tree().join("a.txt"), "changed by a line\n").unwrap();
    std::fs::write(o.tree().join("loose"), "made\n").unwrap();
    o
}

#[test]
fn at_its_own_commit_it_keeps_tracked_changes_and_untracked_files_and_says_so() {
    let o = left_with_work();
    let out = start_keeping(&o, BOX2, "moor/two");
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("moor: work tree refreshed\n"), "{text}");
    assert!(
        !text.contains("moor.clone=base"),
        "the tree is dirty: {text}"
    );
    assert_eq!(read(&o, "a.txt"), "changed by a line\n");
    assert_eq!(read(&o, "loose"), "made\n");
}

#[test]
fn it_drops_the_source_box_s_branch() {
    let o = left_with_work();
    start_keeping(&o, BOX2, "moor/two");
    let heads = tree_git(&o, &["for-each-ref", "--format=%(refname)", "refs/heads"]);
    assert_eq!(heads, "refs/heads/moor/two");
}

#[test]
fn a_pushed_branch_brings_its_commits_back_resets_tracked_files_and_keeps_untracked_ones() {
    let o = left_with_work();
    let tip = o.push(BRANCH, &[("b.txt", "pushed\n")]);
    let out = start_keeping(&o, BOX2, BRANCH);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("moor.clone=base"), "off the base: {text}");
    assert_eq!(tree_git(&o, &["rev-parse", "HEAD"]), tip);
    assert_eq!(read(&o, "b.txt"), "pushed\n");
    assert_eq!(
        read(&o, "a.txt"),
        "one\n",
        "a line runs again and makes it anew"
    );
    assert_eq!(read(&o, "loose"), "made\n");
}

/// Makes the tree's `git fetch` fail, as a lost network does.
fn break_fetch(o: &Origin) {
    tree_git(o, &["config", "remote.origin.fetch", "not-a-refspec"]);
}

fn text(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn a_fallback_to_a_fresh_clone_says_so_on_every_attempt_of_that_box_and_no_later_box() {
    let o = left_with_work();
    break_fetch(&o);
    let first = text(&start_keeping(&o, BOX2, "moor/two"));
    assert!(first.contains("moor: work tree cold after:"), "{first}");
    assert!(went_cold(&first), "{first}");
    let again = text(&start_keeping(&o, BOX2, "moor/two"));
    assert_eq!(outcome(&again), None, "{again}");
    assert!(went_cold(&again), "a restart must still know: {again}");
    let later = text(&start_keeping(&o, "moor-3-cccccc", "moor/three"));
    assert!(!went_cold(&later), "{later}");
}

#[test]
fn a_refresh_that_is_not_a_fallback_never_says_so() {
    let o = left_with_work();
    let first = text(&start_keeping(&o, BOX2, "moor/two"));
    assert!(!went_cold(&first), "{first}");
    assert!(!went_cold(&text(&start_keeping(&o, BOX2, "moor/two"))));
}

#[test]
fn a_wipe_a_restart_cut_short_finishes_on_the_next_attempt_and_says_so() {
    let o = left_with_work();
    std::fs::create_dir_all(o.tree().join(".moor")).unwrap();
    std::fs::write(o.tree().join(".moor/tree-cold"), format!("{BOX2}\n")).unwrap();
    let out = text(&start_keeping(&o, BOX2, "moor/two"));
    assert!(went_cold(&out), "{out}");
    assert!(!o.tree().join("loose").exists(), "the wipe ran to its end");
}

#[test]
fn a_tag_moved_on_the_remote_does_not_send_the_tree_cold() {
    let o = Origin::new("moved-tag");
    let seed = o.seed();
    o.git(&seed, &["tag", "nightly"]);
    o.git(&seed, &["push", "-q", "origin", "main", "nightly"]);
    ok(&o, BOX1, &o.base);
    std::fs::write(o.tree().join("loose"), "made\n").unwrap();
    std::fs::write(seed.join("b.txt"), "moved\n").unwrap();
    o.git(&seed, &["commit", "-qam", "moved"]);
    o.git(&seed, &["tag", "-f", "nightly"]);
    o.git(&seed, &["push", "-qf", "origin", "main", "nightly"]);
    let moved = o.git(&seed, &["rev-parse", "HEAD"]);
    let out = text(&start_keeping(&o, BOX2, "moor/two"));
    assert_eq!(outcome(&out), Some("refreshed"), "{out}");
    assert_eq!(read(&o, "loose"), "made\n");
    assert_eq!(tree_git(&o, &["rev-parse", "nightly"]), moved);
}
