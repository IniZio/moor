//! The script run under `sh` with real git, in a partial clone with build outputs left behind.

use super::{head_at, parse, script};
use crate::testgit::Origin;

const LIMIT: usize = 3;

/// A work tree whose `.gitignore` ignores `dist/` and `*.log`; its first commit.
fn tree() -> (Origin, String) {
    let o = Origin::new("warm-disk");
    let t = o.tree();
    let url = o.url();
    o.git(&t, &["clone", "-q", "--filter=blob:none", &url, "."]);
    std::fs::write(t.join(".gitignore"), "dist/\n.moor/\n*.log\n").unwrap();
    o.git(&t, &["add", "."]);
    o.git(&t, &["commit", "-qm", "ignore"]);
    let first = o.git(&t, &["rev-parse", "HEAD"]);
    (o, first)
}

/// The script's parsed output at the tree's HEAD, against `commit`.
fn seen(o: &Origin, commit: &str) -> (Option<u64>, Vec<String>) {
    let out = o.sh(&o.tree(), &script(commit, LIMIT), &[]);
    assert!(out.status.success(), "{out:?}");
    parse(&String::from_utf8(out.stdout).unwrap())
}

#[test]
fn it_counts_the_tracked_paths_changed_since_the_disk_s_commit() {
    let (o, first) = tree();
    let t = o.tree();
    assert_eq!(seen(&o, &first).0, Some(0));
    std::fs::write(t.join("x.txt"), "x").unwrap();
    std::fs::write(t.join("y.txt"), "y").unwrap();
    o.git(&t, &["add", "."]);
    o.git(&t, &["commit", "-qm", "two"]);
    assert_eq!(seen(&o, &first).0, Some(2));
}

#[test]
fn an_unknown_commit_gives_no_count_and_still_exits_0() {
    let (o, _) = tree();
    assert_eq!(seen(&o, "0123456789012345678901234567890123456789").0, None);
}

#[test]
fn it_names_ignored_paths_at_their_top_without_moor_s_own() {
    let (o, first) = tree();
    let t = o.tree();
    std::fs::create_dir_all(t.join("dist/deep")).unwrap();
    std::fs::write(t.join("dist/deep/out"), "o").unwrap();
    std::fs::create_dir_all(t.join("sub/dist")).unwrap();
    std::fs::write(t.join("sub/kept"), "k").unwrap();
    o.git(&t, &["add", "sub/kept"]);
    o.git(&t, &["commit", "-qm", "sub"]);
    std::fs::write(t.join("sub/dist/out"), "o").unwrap();
    std::fs::write(t.join("a.log"), "l").unwrap();
    std::fs::create_dir_all(t.join(".moor")).unwrap();
    std::fs::write(t.join(".moor/tree"), "k").unwrap();
    let (_, mut ignored) = seen(&o, &first);
    ignored.sort();
    assert_eq!(ignored, ["a.log", "dist/", "sub/dist/"]);
}

#[test]
fn it_prints_no_more_paths_than_the_limit() {
    let (o, first) = tree();
    let t = o.tree();
    for i in 0..LIMIT + 5 {
        std::fs::write(t.join(format!("{i}.log")), "l").unwrap();
    }
    assert_eq!(seen(&o, &first).1.len(), LIMIT);
}

#[test]
fn head_at_passes_only_while_head_is_the_commit() {
    let (o, first) = tree();
    let t = o.tree();
    assert!(o.sh(&t, &head_at(&first), &[]).status.success());
    o.git(&t, &["commit", "-qm", "moved", "--allow-empty"]);
    assert!(!o.sh(&t, &head_at(&first), &[]).status.success());
}

#[test]
fn head_at_refuses_when_it_can_read_neither_commit() {
    let o = Origin::new("no-repo");
    let not_a_repo = o.path("empty");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    assert!(!o.sh(&not_a_repo, &head_at("0123456"), &[]).status.success());
}
