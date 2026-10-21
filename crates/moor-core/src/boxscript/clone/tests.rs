//! The clone script runs under `sh` with real git against a local bare repo reached by `file://`.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{clone_script, Checkout, CloneStep};

const BRANCH: &str = "moor/demo";

/// A scratch dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("moor-clone-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A scratch dir holding a bare remote with one commit on `main`, and an empty Box workdir.
struct Remote {
    dir: Scratch,
    base: String,
}

impl Remote {
    fn new(name: &str) -> Self {
        let dir = Scratch::new(name);
        let r = Self {
            dir,
            base: String::new(),
        };
        r.git(r.dir.path(), &["init", "-q", "-b", "main", "seed"]);
        std::fs::write(r.path("seed").join("a.txt"), "one\n").unwrap();
        r.git(&r.path("seed"), &["add", "a.txt"]);
        r.git(&r.path("seed"), &["commit", "-q", "-m", "one"]);
        r.git(
            r.dir.path(),
            &["clone", "-q", "--bare", "seed", "remote.git"],
        );
        r.git(
            &r.path("remote.git"),
            &["config", "uploadpack.allowFilter", "true"],
        );
        std::fs::create_dir(r.path("box")).unwrap();
        let base = r.git(&r.path("seed"), &["rev-parse", "HEAD"]);
        Self { base, ..r }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn url(&self) -> String {
        format!("file://{}", self.path("remote.git").display())
    }

    /// Runs git in `dir` with no user or system config, and returns its trimmed stdout.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@x.io",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Commits a change to `a.txt` in the seed and pushes it to `refs/heads/<branch>` on the remote.
    fn push_branch(&self) -> String {
        let seed = self.path("seed");
        std::fs::write(seed.join("a.txt"), "two\n").unwrap();
        self.git(&seed, &["commit", "-q", "-am", "two"]);
        self.git(
            &seed,
            &[
                "push",
                "-q",
                "../remote.git",
                &format!("HEAD:refs/heads/{BRANCH}"),
            ],
        );
        self.git(&seed, &["rev-parse", "HEAD"])
    }

    /// Runs the clone script for `base` in the Box workdir and returns its exit code.
    fn run(&self, base: &str) -> i32 {
        let url = self.url();
        let script = clone_script(&Checkout {
            url: &url,
            base,
            branch: BRANCH,
        });
        let out = Command::new("sh")
            .args(["-c", &script])
            .current_dir(self.path("box"))
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        out.status.code().unwrap_or(-1)
    }

    /// What a rerun must leave alone.
    fn state(&self) -> State {
        let b = self.path("box");
        State {
            branch: self.git(&b, &["symbolic-ref", "--short", "HEAD"]),
            head: self.git(&b, &["rev-parse", "HEAD"]),
            refs: self.git(&b, &["for-each-ref"]),
            reflog: self.git(&b, &["reflog", "show", "HEAD"]),
            status: self.git(&b, &["status", "--porcelain"]),
            exclude: std::fs::read_to_string(b.join(".git/info/exclude")).unwrap(),
        }
    }
}

/// The Box checkout as a rerun of the script could change it.
#[derive(Debug, PartialEq, Eq)]
struct State {
    branch: String,
    head: String,
    refs: String,
    reflog: String,
    status: String,
    exclude: String,
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[test]
fn the_first_run_is_a_partial_clone_on_the_box_branch_at_the_base() {
    if !git_available() {
        return;
    }
    let r = Remote::new("first");
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), r.base);
    assert_eq!(r.git(&b, &["remote", "get-url", "origin"]), r.url());
    assert_eq!(
        r.git(&b, &["config", "remote.origin.partialclonefilter"]),
        "blob:none"
    );
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "one\n");
    assert_eq!(r.state().branch, BRANCH);
}

#[test]
fn a_second_run_changes_nothing() {
    if !git_available() {
        return;
    }
    let r = Remote::new("twice");
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    std::fs::create_dir_all(b.join(".moor")).unwrap();
    std::fs::write(b.join(".moor/setup.log"), "x").unwrap();
    let first = r.state();
    assert_eq!(first.status, "", ".moor/ is excluded");
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.state(), first);
}

#[test]
fn a_branch_on_the_remote_is_checked_out_instead_of_the_base() {
    if !git_available() {
        return;
    }
    let r = Remote::new("branch");
    let tip = r.push_branch();
    assert_ne!(tip, r.base);
    assert_eq!(r.run(&r.base), 0);
    let b = r.path("box");
    assert_eq!(r.git(&b, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), tip);
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "two\n");
}

#[test]
fn a_clone_stopped_before_its_checkout_carries_on() {
    if !git_available() {
        return;
    }
    let r = Remote::new("resume");
    let b = r.path("box");
    r.git(&b, &["clone", "-q", "--no-checkout", &r.url(), "."]);
    assert_eq!(r.run(&r.base), 0);
    assert_eq!(r.git(&b, &["symbolic-ref", "--short", "HEAD"]), BRANCH);
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), r.base);
}

#[test]
fn a_base_that_is_not_on_the_remote_stops_at_the_base_step() {
    if !git_available() {
        return;
    }
    let r = Remote::new("nobase");
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let code = r.run(missing);
    assert_eq!(
        CloneStep::of_exit(code),
        Some(CloneStep::Base),
        "exit {code}"
    );
}

#[test]
fn an_unreachable_url_stops_at_the_clone_step() {
    let script = clone_script(&Checkout {
        url: "file:///nonexistent/moor-clone-test.git",
        base: "0123456789abcdef0123456789abcdef01234567",
        branch: BRANCH,
    });
    let dir = Scratch::new("unreachable");
    let out = Command::new("sh")
        .args(["-c", &script])
        .current_dir(dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code().and_then(CloneStep::of_exit),
        Some(CloneStep::Clone),
        "{out:?}"
    );
}

#[test]
fn values_are_quoted() {
    let script = clone_script(&Checkout {
        url: "https://github.com/o/n.git",
        base: "$(touch x)",
        branch: "b'; rm -rf /; '",
    });
    assert!(script.contains("'$(touch x)'"), "{script}");
    assert!(script.contains(r"'b'\''; rm -rf /; '\'''"), "{script}");
}
