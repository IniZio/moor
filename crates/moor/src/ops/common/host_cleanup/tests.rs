use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{box_name, script};
use moor_core::BoxHandle;

/// A host home and a crabbox work root in one tempdir.
struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let h = Self { dir };
        fs::create_dir_all(h.home()).unwrap();
        fs::create_dir_all(h.root()).unwrap();
        fs::write(h.home().join("keep"), "mine").unwrap();
        h
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    /// Creates a Task's lease, cred and tmp dirs; returns the workdir.
    fn task(&self, id: &str, name: &str) -> String {
        let work = self.root().join(name).join("repo");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("f"), "x").unwrap();
        for d in [format!(".moor-cred-{id}-n1"), format!(".moor-tmp-{id}-n1")] {
            fs::create_dir_all(self.home().join(&d)).unwrap();
            fs::write(self.home().join(&d).join("s"), "secret").unwrap();
        }
        work.display().to_string()
    }

    fn alive(&self, id: &str, name: &str) -> [bool; 3] {
        [
            self.root().join(name).exists(),
            self.home().join(format!(".moor-cred-{id}-n1")).exists(),
            self.home().join(format!(".moor-tmp-{id}-n1")).exists(),
        ]
    }

    fn sh(&self, script: &str) {
        let st = Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", self.home())
            .status()
            .unwrap();
        assert!(st.success());
    }
}

#[test]
fn removes_only_the_tasks_own_paths() {
    let h = Host::new();
    let a = h.task("1", "moor-a-x");
    h.task("10", "moor-b-y");
    h.task("2", "moor-c-z");
    h.sh(&script("1", "moor-a-x", &a).unwrap());
    assert_eq!(h.alive("1", "moor-a-x"), [false; 3]);
    assert_eq!(h.alive("10", "moor-b-y"), [true; 3]);
    assert_eq!(h.alive("2", "moor-c-z"), [true; 3]);
    assert_eq!(fs::read_to_string(h.home().join("keep")).unwrap(), "mine");
    assert!(h.root().is_dir() && h.home().is_dir());
}

#[test]
fn a_prefix_task_id_keeps_the_longer_ones() {
    let h = Host::new();
    let long = h.task("10", "moor-b-y");
    h.task("1", "moor-a-x");
    h.sh(&script("10", "moor-b-y", &long).unwrap());
    assert_eq!(h.alive("10", "moor-b-y"), [false; 3]);
    assert_eq!(h.alive("1", "moor-a-x"), [true; 3]);
}

#[test]
fn a_workdir_outside_the_lease_keeps_the_lease_dir() {
    let h = Host::new();
    let w = h.task("1", "moor-a-x");
    h.sh(&script("1", "moor-other-x", &w).unwrap());
    assert_eq!(h.alive("1", "moor-a-x"), [true, false, false]);
    assert!(Path::new(&w).is_dir());
}

#[test]
fn a_lease_dir_is_never_a_shallow_or_dotted_path() {
    for w in [
        "/moor-a-x/repo",
        "/r/moor-a-x/../repo",
        "/r//moor-a-x/repo",
        "/moor-a-x",
    ] {
        let s = script("1", "moor-a-x", w).unwrap();
        assert_eq!(s.lines().count(), 3, "{w}: {s}");
    }
}

#[test]
fn refuses_unsafe_input() {
    for id in [
        "", ".", "..", "t 1", "t/1", "t*", "t;rm", "$HOME", "t\n", "'",
    ] {
        assert!(
            script(id, "moor-a-x", "/r/moor-a-x/repo").is_none(),
            "{id:?}"
        );
        assert!(script("1", id, "/r/moor-a-x/repo").is_none(), "{id:?}");
    }
    assert!(script("1", "moor-a-x", "r/moor-a-x/repo").is_none());
    assert!(script("1", "moor-a-x", "").is_none());
}

#[test]
fn a_task_id_is_ascii_digits_only() {
    for id in ["1-5", "1.5", "-1", "1_0", "t1", "\u{0661}"] {
        assert!(
            script(id, "moor-a-x", "/r/moor-a-x/repo").is_none(),
            "{id:?}"
        );
    }
    assert!(script("07", "moor-a-x", "/r/moor-a-x/repo").is_some());
}

#[test]
fn quotes_a_workdir_with_odd_characters() {
    let h = Host::new();
    let work = h.root().join("moor-a-x").join("it's a repo");
    fs::create_dir_all(&work).unwrap();
    h.sh(&script("1", "moor-a-x", &work.display().to_string()).unwrap());
    assert!(!h.root().join("moor-a-x").exists());
}

#[test]
fn an_empty_home_removes_nothing() {
    let h = Host::new();
    let w = h.task("1", "moor-a-x");
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("1", "moor-a-x", &w).unwrap())
        .env("HOME", "")
        .current_dir(h.dir.path())
        .status()
        .unwrap();
    assert!(!st.success());
    assert_eq!(h.alive("1", "moor-a-x"), [true; 3]);
}

#[test]
fn box_name_is_the_part_after_the_provider() {
    assert_eq!(box_name(&BoxHandle::new("ssh/moor-t-n")), "moor-t-n");
    assert_eq!(box_name(&BoxHandle::new("box-1")), "box-1");
}

#[test]
fn the_lease_removal_is_guarded_by_home() {
    let s = script("7", "moor-t-n1", "/srv/crabbox/moor-t-n1/repo").unwrap();
    assert_eq!(
        s,
        concat!(
            "[ -n \"$HOME\" ] && [ \"$HOME\" != / ] || exit 1\n",
            "rm -rf -- \"$HOME\"/.moor-cred-7-* \"$HOME\"/.moor-tmp-7-*\n",
            "case \"$HOME/\" in '/srv/crabbox/moor-t-n1/repo.moor-home'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1/repo.moor-home' ;; esac\n",
            "case \"$HOME/\" in '/srv/crabbox/moor-t-n1'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1' ;; esac"
        )
    );
}

#[test]
fn a_box_name_without_the_moor_prefix_keeps_the_lease() {
    for name in ["newman", "moor-", "moo-x"] {
        let h = Host::new();
        let work = h.task("1", name);
        h.sh(&script("1", name, &work).unwrap());
        assert_eq!(h.alive("1", name), [true, false, false], "{name}");
    }
}

#[test]
fn a_lease_that_is_home_survives() {
    let h = Host::new();
    let home = h.root().join("moor-x");
    fs::create_dir_all(home.join("repo")).unwrap();
    fs::write(home.join("keep"), "mine").unwrap();
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("1", "moor-x", &home.join("repo").display().to_string()).unwrap())
        .env("HOME", &home)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(home.join("keep").exists() && home.join("repo").exists());
}

#[test]
fn a_lease_that_contains_home_survives() {
    let h = Host::new();
    let lease = h.root().join("moor-x");
    let home = lease.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("keep"), "mine").unwrap();
    fs::create_dir_all(lease.join("repo")).unwrap();
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("1", "moor-x", &lease.join("repo").display().to_string()).unwrap())
        .env("HOME", &home)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(home.join("keep").exists());
}

#[test]
fn removes_the_per_task_home_even_when_the_lease_is_kept() {
    let h = Host::new();
    let w = h.task("1", "moor-a-x");
    let agent_home = format!("{w}.moor-home");
    fs::create_dir_all(Path::new(&agent_home).join(".claude")).unwrap();
    fs::write(Path::new(&agent_home).join(".claude/s"), "secret").unwrap();
    h.sh(&script("1", "moor-other-x", &w).unwrap());
    assert!(!Path::new(&agent_home).exists());
    assert!(Path::new(&w).is_dir());
}

#[test]
fn removes_the_per_task_home_with_the_lease() {
    let h = Host::new();
    let w = h.task("1", "moor-a-x");
    fs::create_dir_all(format!("{w}.moor-home")).unwrap();
    h.sh(&script("1", "moor-a-x", &w).unwrap());
    assert_eq!(h.alive("1", "moor-a-x"), [false; 3]);
}

#[test]
fn a_relative_or_empty_workdir_emits_no_home_removal() {
    for w in ["", "repo", "r/moor-a-x/repo"] {
        assert!(script("1", "moor-a-x", w).is_none(), "{w:?}");
    }
}

#[test]
fn a_per_task_home_that_is_or_contains_home_survives() {
    for sub in ["", "inner"] {
        let h = Host::new();
        let work = h.root().join("moor-x").join("repo");
        let agent_home = h.root().join("moor-x").join("repo.moor-home");
        let home = agent_home.join(sub);
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("keep"), "mine").unwrap();
        let st = Command::new("sh")
            .arg("-c")
            .arg(script("1", "moor-x", &work.display().to_string()).unwrap())
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(st.success());
        assert!(home.join("keep").exists(), "{sub:?}");
    }
}
