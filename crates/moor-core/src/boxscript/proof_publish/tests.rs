//! Tests for the proof publish script, run under real `sh` and `git` in scratch dirs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::{FILE, SCRIPT};

struct Scratch {
    root: PathBuf,
    work: PathBuf,
    bare: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

impl Scratch {
    /// A work repo with one commit and a `moor` remote named like GitHub, backed by a bare repo.
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("moor-pubproof-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let work = root.join("work");
        let bare = root.join("bare.git");
        fs::create_dir_all(work.join(".moor/proof")).unwrap();
        fs::create_dir_all(&bare).unwrap();
        git_in(&bare, &["init", "--quiet", "--bare"]);
        git_in(&work, &["init", "--quiet", "-b", "task"]);
        fs::write(work.join("README"), "hi").unwrap();
        fs::write(work.join(".gitignore"), ".moor/\n").unwrap();
        git_in(&work, &["add", "README", ".gitignore"]);
        git_in(
            &work,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "init",
            ],
        );
        git_in(
            &work,
            &[
                "remote",
                "add",
                "moor",
                "https://github.com/acme/widgets.git",
            ],
        );
        fs::write(work.join(FILE), SCRIPT).unwrap();
        Self { root, work, bare }
    }

    fn proof(&self, name: &str, body: &[u8]) {
        fs::write(self.work.join(".moor/proof").join(name), body).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new("sh")
            .arg(FILE)
            .args(args)
            .current_dir(&self.work)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.{}.insteadOf", self.bare.display()),
            )
            .env("GIT_CONFIG_VALUE_0", "https://github.com/acme/widgets.git")
            .output()
            .unwrap()
    }

    fn ok(&self, task: &str) -> String {
        let out = self.run(&[task]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn bare_ref(&self, branch: &str) -> String {
        git_in(&self.bare, &["rev-parse", &format!("refs/heads/{branch}")])
    }

    fn state(&self) -> [String; 5] {
        [
            git_in(&self.work, &["rev-parse", "HEAD"]),
            git_in(&self.work, &["rev-parse", "refs/heads/task"]),
            git_in(&self.work, &["status", "--porcelain", "--ignored"]),
            git_in(&self.work, &["ls-files", "-s"]),
            git_in(&self.work, &["branch", "--list"]),
        ]
    }
}

#[test]
fn first_run_is_an_orphan_commit() {
    let s = Scratch::new("orphan");
    s.proof("a.png", b"abc");
    s.ok("t1");
    let sha = s.bare_ref("moor-proof/t1");
    assert_eq!(
        git_in(&s.bare, &["rev-list", "--parents", "-n1", &sha]),
        sha
    );
    assert_eq!(git_in(&s.bare, &["ls-tree", "--name-only", &sha]), "a.png");
}

#[test]
fn second_run_fast_forwards() {
    let s = Scratch::new("ff");
    s.proof("a.png", b"abc");
    s.ok("t1");
    let first = s.bare_ref("moor-proof/t1");
    s.proof("b.mp4", b"vid");
    s.ok("t1");
    let second = s.bare_ref("moor-proof/t1");
    assert_ne!(first, second);
    assert_eq!(
        git_in(&s.bare, &["rev-parse", &format!("{second}^")]),
        first
    );
    assert_eq!(
        git_in(&s.bare, &["ls-tree", "--name-only", &second]),
        "a.png\nb.mp4"
    );
}

#[test]
fn rerun_without_changes_reuses_the_tip() {
    let s = Scratch::new("same");
    s.proof("a.png", b"abc");
    s.ok("t1");
    let first = s.bare_ref("moor-proof/t1");
    s.ok("t1");
    assert_eq!(s.bare_ref("moor-proof/t1"), first);
}

#[test]
fn head_branch_worktree_and_index_are_untouched() {
    let s = Scratch::new("pure");
    s.proof("a.png", b"abc");
    fs::write(s.work.join("dirty"), "x").unwrap();
    git_in(&s.work, &["add", "dirty"]);
    let before = s.state();
    s.ok("t1");
    assert_eq!(s.state(), before);
}

#[test]
fn prints_markdown_pinned_to_the_commit() {
    let s = Scratch::new("md");
    s.proof("Shot 1(a)#?.PNG", b"abc");
    s.proof("clip.mp4", b"vid");
    s.proof("notes.txt", b"n");
    let out = s.ok("t1");
    let sha = s.bare_ref("moor-proof/t1");
    let base = format!("https://github.com/acme/widgets/blob/{sha}");
    assert_eq!(
        out,
        format!(
            "![Shot 1(a)#?.PNG]({base}/Shot%201%28a%29%23%3F.PNG?raw=true)\n\
             [clip.mp4]({base}/clip.mp4)\n\
             [notes.txt]({base}/notes.txt)\n"
        )
    );
}

#[test]
fn bad_task_name_is_refused() {
    let s = Scratch::new("badtask");
    s.proof("a.png", b"abc");
    for bad in ["", "a b", "x..y", "a:b"] {
        let out = s.run(&[bad]);
        assert!(!out.status.success(), "{bad:?} accepted");
    }
    assert!(Command::new("git")
        .args(["for-each-ref", "refs/heads"])
        .current_dir(&s.bare)
        .output()
        .unwrap()
        .stdout
        .is_empty());
}

#[test]
fn empty_proof_dir_is_refused() {
    let s = Scratch::new("empty");
    fs::create_dir(s.work.join(".moor/proof/sub")).unwrap();
    let out = s.run(&["t1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no regular files"));
}

#[test]
fn symlinks_are_skipped() {
    let s = Scratch::new("lnk");
    s.proof("a.png", b"abc");
    std::os::unix::fs::symlink("/etc/hostname", s.work.join(".moor/proof/link.png")).unwrap();
    let out = s.ok("t1");
    assert!(!out.contains("link.png"));
    let sha = s.bare_ref("moor-proof/t1");
    assert_eq!(git_in(&s.bare, &["ls-tree", "--name-only", &sha]), "a.png");
}

#[test]
fn unparsable_remote_is_refused() {
    let s = Scratch::new("url");
    s.proof("a.png", b"abc");
    git_in(
        &s.work,
        &["remote", "set-url", "moor", "https://example.com/x"],
    );
    assert!(!s.run(&["t1"]).status.success());
}
