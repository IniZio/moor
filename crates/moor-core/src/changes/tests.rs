use std::path::Path;

use super::*;
use crate::testgit::{Origin, BRANCH};

/// A work tree cloned from `o` on the Box branch.
fn cloned(o: &Origin) -> Tree {
    let tree = o.tree();
    o.git(&tree, &["clone", "-q", &o.url(), "."]);
    o.git(&tree, &["checkout", "-q", "-b", BRANCH]);
    Tree {
        repo: "o/r".to_owned(),
        dir: tree.display().to_string(),
        base: Some("main".to_owned()),
    }
}

/// What `script` printed in `o`'s work tree.
fn run(o: &Origin, script: &str) -> String {
    let out = o.sh(&o.tree(), script, &[]);
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The receipt steps: the turn's files and everything against the base.
fn receipt(o: &Origin, t: &Tree) -> (Vec<FileChange>, Vec<FileChange>) {
    assert_eq!(run(o, &snapshot(t, "r")), "ok\n");
    let turn = parse_numstat(&run(o, &turn_numstat(t, "r")));
    let total = parse_numstat(&run(o, &total_numstat(t, "r", true)));
    (turn, total)
}

fn write(dir: &Path, name: &str, body: &[u8]) {
    std::fs::write(dir.join(name), body).unwrap();
}

fn change(path: &str, added: u64, removed: u64) -> FileChange {
    FileChange {
        path: path.to_owned(),
        from: None,
        added: Some(added),
        removed: Some(removed),
    }
}

#[test]
fn counts_edits_untracked_files_and_commits_against_the_base() {
    let o = Origin::new("changes");
    let t = cloned(&o);
    let dir = o.tree();
    write(&dir, "a.txt", b"one\ntwo\n");
    o.git(&dir, &["commit", "-qam", "two"]);
    write(&dir, "c.txt", b"new\nfile\n");
    std::fs::remove_file(dir.join("b.txt")).unwrap();
    let (turn, total) = receipt(&o, &t);
    let want = vec![
        change("a.txt", 1, 0),
        change("b.txt", 0, 1),
        change("c.txt", 2, 0),
    ];
    assert_eq!(total, want);
    assert_eq!(turn, want, "the first turn counts from the base");
    assert_eq!(
        Tally::of(&total),
        Tally {
            files: 3,
            added: 3,
            removed: 1
        }
    );
    assert_eq!(
        o.git(&dir, &["diff", "--cached", "--name-only"]),
        "",
        "the index is the agent's"
    );
}

#[test]
fn a_turn_counts_only_what_changed_since_the_last_receipt() {
    let o = Origin::new("turns");
    let t = cloned(&o);
    let dir = o.tree();
    write(&dir, "a.txt", b"one\ntwo\n");
    receipt(&o, &t);
    write(&dir, "c.txt", b"x\n");
    let (turn, total) = receipt(&o, &t);
    assert_eq!(turn, vec![change("c.txt", 1, 0)]);
    assert_eq!(total, vec![change("a.txt", 1, 0), change("c.txt", 1, 0)]);
}

#[test]
fn marks_renames_and_binary_files_and_skips_moor_files() {
    let o = Origin::new("renames");
    let t = cloned(&o);
    let dir = o.tree();
    std::fs::create_dir_all(dir.join(".moor/artifacts")).unwrap();
    std::fs::write(dir.join(".git/info/exclude"), ".moor/\n").unwrap();
    write(&dir, ".moor/artifacts/shot.png", b"\x89PNG\0\0");
    o.git(&dir, &["mv", "b.txt", "moved.txt"]);
    write(&dir, "blob.bin", b"\0\x01\x02");
    let (_, total) = receipt(&o, &t);
    let binary = FileChange {
        path: "blob.bin".to_owned(),
        from: None,
        added: None,
        removed: None,
    };
    let moved = FileChange {
        from: Some("b.txt".to_owned()),
        ..change("moved.txt", 0, 0)
    };
    assert_eq!(total, vec![binary, moved]);
}

#[test]
fn a_patch_shows_new_files_and_is_cut_at_a_whole_line() {
    let o = Origin::new("patch");
    let t = cloned(&o);
    write(&o.tree(), "c.txt", b"new\nfile\n");
    assert_eq!(run(&o, &snapshot(&t, "v1")), "ok\n");
    let full = run(&o, &patch(&t, "v1", PATCH_BYTES));
    assert!(full.contains("new file mode"), "{full}");
    assert!(full.contains("+new\n+file\n"), "{full}");
    assert_eq!(capped(&full, PATCH_BYTES), (full.clone(), false));
    let (cut, was) = capped(&full, full.len() - 3);
    assert!(was && cut.ends_with("+new\n"), "{cut}");
    let left: Vec<String> = Origin::entries(&o.tree().join(".git/moor"));
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_missing_checkout_says_why() {
    let o = Origin::new("missing");
    let t = Tree {
        repo: "o/r".to_owned(),
        dir: o.tree().join("nope").display().to_string(),
        base: None,
    };
    assert_eq!(run(&o, &snapshot(&t, "r")), "no checkout\n");
    assert_eq!(run(&o, &turn_numstat(&t, "r")), "");
}

#[test]
fn reads_renames_from_the_nul_separated_form() {
    let out = concat!("1\t2\ta b.txt\0", "-\t-\tpic.png\0", "0\t0\t\0old\0new\0");
    let [plain, binary, renamed] = parse_numstat(out).try_into().unwrap();
    assert_eq!(plain, change("a b.txt", 1, 2));
    assert_eq!(binary.added, None);
    assert_eq!(renamed.from.as_deref(), Some("old"));
    assert_eq!(renamed.path, "new");
}
