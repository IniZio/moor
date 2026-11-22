//! The clone through a repo's bare git mirror, under `sh` with real git and `flock`.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use super::{Remote, BRANCH};
use crate::boxscript::clone::{clone_script, mirror_skipped, Checkout};
use crate::boxscript::quote;

/// A pack-objects hook that logs how many objects each pack the remote serves holds.
const COUNTING_HOOK: &str = r#"#!/bin/sh
t=$(mktemp)
"$@" >"$t" || exit
od -An -tu1 -j8 -N4 "$t" | tr -s ' ' ' ' >>"$MOOR_TEST_PACKS"
echo >>"$MOOR_TEST_PACKS"
cat "$t"
rm -f "$t"
"#;

impl Remote {
    fn mirror(&self) -> std::path::PathBuf {
        self.path("mirror/o/r.git")
    }

    /// The clone script for the base with the test's mirror, run as a new Box would.
    fn mirrored(&self) -> Command {
        let url = self.url();
        let mirror = quote(&self.mirror().display().to_string());
        let script = clone_script(
            &Checkout {
                url: &url,
                base: &self.base,
                branch: BRANCH,
            },
            Some(&mirror),
        );
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script])
            .current_dir(self.path("box"))
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("MOOR_TEST_PACKS", self.path("packs"));
        cmd
    }

    /// Runs [`Remote::mirrored`] in an empty workdir, like a new Box.
    fn new_box(&self) -> Output {
        let b = self.path("box");
        std::fs::remove_dir_all(&b).unwrap();
        std::fs::create_dir(&b).unwrap();
        self.mirrored().output().unwrap()
    }

    /// Commits `n` changes to `a.txt` on the remote's `main`.
    fn grow(&self, n: usize) {
        let seed = self.path("seed");
        for i in 0..n {
            std::fs::write(seed.join("a.txt"), format!("{i}\n")).unwrap();
            self.git(&seed, &["commit", "-q", "-am", &format!("c{i}")]);
        }
        self.git(
            &seed,
            &["push", "-q", "../remote.git", "HEAD:refs/heads/main"],
        );
    }

    /// Logs the object count of every pack the remote serves from now on.
    fn count_packs(&self) {
        let hook = self.path("count-packs");
        moor_testkit::write_executable(&hook, COUNTING_HOOK).unwrap();
        let home = self.dir.path();
        let hook = hook.display().to_string();
        self.git(
            home,
            &["config", "--global", "uploadpack.packObjectsHook", &hook],
        );
    }

    /// The object count of each pack served since the last call.
    fn packs(&self) -> Vec<u32> {
        let log = std::fs::read_to_string(self.path("packs")).unwrap_or_default();
        let _ = std::fs::remove_file(self.path("packs"));
        log.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                l.split_whitespace()
                    .map(|b| b.parse::<u32>().unwrap())
                    .fold(0, |n, b| (n << 8) | b)
            })
            .collect()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(out: &Output) {
    assert!(out.status.success(), "{}", text(out));
}

#[test]
fn a_first_clone_fills_the_mirror_and_the_box_keeps_no_link_to_it() {
    let r = Remote::new("mirror-first");
    let out = r.new_box();
    ok(&out);
    assert_eq!(mirror_skipped(&text(&out)), None);
    let (b, m) = (r.path("box"), r.mirror());
    assert_eq!(r.git(&m, &["rev-parse", "refs/heads/main"]), r.base);
    assert!(!b.join(".git/objects/info/alternates").exists());
    std::fs::remove_dir_all(&m).unwrap();
    assert_eq!(r.git(&b, &["rev-parse", "HEAD"]), r.base);
    assert_eq!(r.git(&b, &["remote", "get-url", "origin"]), r.url());
    r.git(&b, &["fsck", "--connectivity-only"]);
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "one\n");
}

#[test]
fn the_mirror_config_names_no_remote_and_no_credential() {
    let r = Remote::new("mirror-config");
    ok(&r.new_box());
    let config = r.git(&r.mirror(), &["config", "--local", "--list"]);
    for line in config.lines() {
        assert!(line.starts_with("core."), "{config}");
    }
}

#[test]
fn a_later_box_fetches_only_the_commits_the_mirror_lacks() {
    let r = Remote::new("mirror-delta");
    r.grow(30);
    r.count_packs();
    ok(&r.new_box());
    let first: u32 = r.packs().iter().sum();
    assert!(
        first > 60,
        "the first Box fills the mirror: {first} objects"
    );
    r.grow(1);

    ok(&r.new_box());

    let later = r.packs();
    assert!(
        later.iter().all(|n| *n <= 3),
        "only the new commit, tree and blob: {later:?}"
    );
    let head = r.git(&r.path("seed"), &["rev-parse", "HEAD"]);
    assert_eq!(r.git(&r.path("box"), &["rev-parse", "origin/main"]), head);
}

#[test]
fn a_clone_waits_while_another_box_holds_the_mirror() {
    let r = Remote::new("mirror-lock");
    let m = r.mirror();
    std::fs::create_dir_all(m.parent().unwrap()).unwrap();
    let held = r.path("held");
    let lock = format!("{}.lock", m.display());
    let holder = format!(
        "exec 9>>{}; flock 9; : >{}; sleep 2",
        quote(&lock),
        quote(&held.display().to_string())
    );
    let mut holder =
        moor_testkit::ChildGuard::new(Command::new("sh").args(["-c", &holder]).spawn().unwrap());
    while !held.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut clone = moor_testkit::ChildGuard::new(r.mirrored().spawn().unwrap());
    std::thread::sleep(Duration::from_millis(1000));
    assert!(
        !m.exists(),
        "nothing touches the mirror while another Box holds it"
    );
    assert!(holder.wait().unwrap().success());

    assert!(clone.wait().unwrap().success());
    assert_eq!(r.git(&m, &["rev-parse", "refs/heads/main"]), r.base);
}

#[test]
fn a_ref_lock_a_killed_fetch_left_is_cleared() {
    let r = Remote::new("mirror-stale-lock");
    ok(&r.new_box());
    std::fs::write(r.mirror().join("refs/heads/main.lock"), "").unwrap();
    r.grow(1);

    let out = r.new_box();

    ok(&out);
    assert_eq!(mirror_skipped(&text(&out)), None, "{}", text(&out));
    let head = r.git(&r.path("seed"), &["rev-parse", "HEAD"]);
    assert_eq!(r.git(&r.mirror(), &["rev-parse", "refs/heads/main"]), head);
}

#[test]
fn a_mirror_git_cannot_use_is_named_and_the_box_clones_from_the_remote() {
    let r = Remote::new("mirror-broken");
    let m = r.mirror();
    std::fs::create_dir_all(m.parent().unwrap()).unwrap();
    std::fs::write(&m, "not a repo").unwrap();

    let out = r.new_box();

    ok(&out);
    assert_eq!(mirror_skipped(&text(&out)), Some("git init failed"));
    assert_eq!(r.git(&r.path("box"), &["rev-parse", "HEAD"]), r.base);
}

#[test]
fn without_flock_the_box_says_so_and_clones_from_the_remote() {
    let r = Remote::new("mirror-no-flock");
    let bin = r.path("no-flock");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in [
        "git", "sh", "mkdir", "rm", "mv", "find", "cat", "grep", "printf", "date", "cut", "od",
        "tr", "mktemp",
    ] {
        let Ok(found) = which(tool) else { continue };
        std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
    }
    let out = r.mirrored().env("PATH", &bin).output().unwrap();

    ok(&out);
    assert_eq!(mirror_skipped(&text(&out)), Some("flock is missing"));
    assert!(!r.mirror().exists());
}

fn which(tool: &str) -> Result<std::path::PathBuf, ()> {
    let path = std::env::var("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|d| d.join(tool))
        .find(|p| Path::new(p).is_file())
        .ok_or(())
}
