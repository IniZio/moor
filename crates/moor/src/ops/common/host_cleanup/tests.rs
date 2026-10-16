use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

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
        for d in [
            format!(".moor-cred-{id}-abc123"),
            format!(".moor-tmp-{id}-abc123"),
        ] {
            fs::create_dir_all(self.home().join(&d)).unwrap();
            fs::write(self.home().join(&d).join("s"), "secret").unwrap();
        }
        work.display().to_string()
    }

    fn alive(&self, id: &str, name: &str) -> [bool; 3] {
        [
            self.root().join(name).exists(),
            self.home().join(format!(".moor-cred-{id}-abc123")).exists(),
            self.home().join(format!(".moor-tmp-{id}-abc123")).exists(),
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
    let a = h.task("t1", "moor-a-x");
    h.task("t10", "moor-b-y");
    h.task("t2", "moor-c-z");
    h.sh(&script("t1", "moor-a-x", &a).unwrap());
    assert_eq!(h.alive("t1", "moor-a-x"), [false; 3]);
    assert_eq!(h.alive("t10", "moor-b-y"), [true; 3]);
    assert_eq!(h.alive("t2", "moor-c-z"), [true; 3]);
    assert_eq!(fs::read_to_string(h.home().join("keep")).unwrap(), "mine");
    assert!(h.root().is_dir() && h.home().is_dir());
}

#[test]
fn a_prefix_task_name_keeps_the_longer_ones() {
    let h = Host::new();
    let long = h.task("a-b", "moor-b-y");
    let short = h.task("a", "moor-a-x");
    h.sh(&script("a", "moor-a-x", &short).unwrap());
    assert_eq!(h.alive("a", "moor-a-x"), [false; 3]);
    assert_eq!(h.alive("a-b", "moor-b-y"), [true; 3]);
    h.sh(&script("a-b", "moor-b-y", &long).unwrap());
    assert_eq!(h.alive("a-b", "moor-b-y"), [false; 3]);
}

#[test]
fn the_script_names_the_task_not_an_id() {
    let s = script("fix-it", "moor-a-x", "/r/moor-a-x/repo").unwrap();
    assert!(s.contains(".moor-cred-fix-it-[0-9a-f]"), "{s}");
    assert!(s.contains(".moor-tmp-fix-it-[0-9a-f]"), "{s}");
    assert!(!s.contains("-*"), "{s}");
}

#[test]
fn the_script_kills_only_exact_path_matches() {
    let s = script("t", "moor-a-x", "/r/moor-a-x/repo").unwrap();
    assert!(s.contains("ps -axo pid=,command="), "{s}");
    assert!(s.contains(r#"index($0, ENVIRON["P"])"#), "{s}");
    assert!(s.contains(r#"P="$d/""#), "{s}");
    assert!(s.contains(r#"$1 != ENVIRON["S"]"#), "{s}");
    let term = s.find("kill -TERM").expect("TERM");
    let kill = s.find("kill -KILL").expect("KILL");
    let rm = s.find("rm -rf -- \"$HOME\"").expect("rm");
    assert!(term < kill && kill < rm, "{s}");
}

fn spawn_with_arg(path: &Path) -> Child {
    Command::new("sh")
        .args(["-c", "while :; do sleep 1; done"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn processes_under_the_cred_dir_die_and_others_live() {
    let h = Host::new();
    let w = h.task("t", "moor-a-x");
    h.task("t-b", "moor-b-y");
    let mine = h.home().join(".moor-cred-t-abc123").join("run");
    let other = h.home().join(".moor-cred-t-b-abc123").join("run");
    let mut victim = spawn_with_arg(&mine);
    let mut bystander = spawn_with_arg(&other);
    sleep(Duration::from_millis(300));
    h.sh(&script("t", "moor-a-x", &w).unwrap());
    let mut dead = false;
    for _ in 0..50 {
        if victim.try_wait().unwrap().is_some() {
            dead = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    let alive = bystander.try_wait().unwrap().is_none();
    bystander.kill().unwrap();
    bystander.wait().unwrap();
    if !dead {
        victim.kill().unwrap();
        victim.wait().unwrap();
    }
    assert!(dead, "process under the cred dir survived");
    assert!(alive, "process of another Task was killed");
    assert_eq!(h.alive("t", "moor-a-x"), [false; 3]);
    assert_eq!(h.alive("t-b", "moor-b-y"), [true, true, true]);
}

#[test]
fn a_workdir_outside_the_lease_keeps_the_lease_dir() {
    let h = Host::new();
    let w = h.task("t1", "moor-a-x");
    h.sh(&script("t1", "moor-other-x", &w).unwrap());
    assert_eq!(h.alive("t1", "moor-a-x"), [true, false, false]);
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
        let s = script("t1", "moor-a-x", w).unwrap();
        assert_eq!(s.matches("esac").count(), 1, "{w}: {s}");
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
        assert!(script("t1", id, "/r/moor-a-x/repo").is_none(), "{id:?}");
    }
    assert!(script("t1", "moor-a-x", "r/moor-a-x/repo").is_none());
    assert!(script("t1", "moor-a-x", "").is_none());
}

#[test]
fn a_task_name_has_the_task_name_shape() {
    for n in [
        "-a",
        "A",
        "a_b",
        "a.b",
        "a b",
        "a*",
        "1\u{661}",
        "é",
        &"a".repeat(41),
    ] {
        assert!(script(n, "moor-a-x", "/r/moor-a-x/repo").is_none(), "{n:?}");
    }
    for n in ["a", "0", "a-b", "07", &"a".repeat(40)] {
        assert!(script(n, "moor-a-x", "/r/moor-a-x/repo").is_some(), "{n:?}");
    }
}

#[test]
fn quotes_a_workdir_with_odd_characters() {
    let h = Host::new();
    let work = h.root().join("moor-a-x").join("it's a repo");
    fs::create_dir_all(&work).unwrap();
    h.sh(&script("t1", "moor-a-x", &work.display().to_string()).unwrap());
    assert!(!h.root().join("moor-a-x").exists());
}

#[test]
fn an_empty_home_removes_nothing() {
    let h = Host::new();
    let w = h.task("t1", "moor-a-x");
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("t1", "moor-a-x", &w).unwrap())
        .env("HOME", "")
        .current_dir(h.dir.path())
        .status()
        .unwrap();
    assert!(!st.success());
    assert_eq!(h.alive("t1", "moor-a-x"), [true; 3]);
}

#[test]
fn box_name_is_the_part_after_the_provider() {
    assert_eq!(box_name(&BoxHandle::new("ssh/moor-t-n")), "moor-t-n");
    assert_eq!(box_name(&BoxHandle::new("box-1")), "box-1");
}

#[test]
fn the_lease_removal_is_guarded_by_home() {
    let s = script("t7", "moor-t-n1", "/srv/crabbox/moor-t-n1/repo").unwrap();
    assert!(
        s.starts_with("[ -n \"$HOME\" ] && [ \"$HOME\" != / ] || exit 1\n"),
        "{s}"
    );
    assert!(
        s.contains("\ncase \"$HOME/\" in '/srv/crabbox/moor-t-n1/repo.moor-home'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1/repo.moor-home' ;; esac\n"),
        "{s}"
    );
    assert!(
        s.ends_with(
            "case \"$HOME/\" in '/srv/crabbox/moor-t-n1'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1' ;; esac"
        ),
        "{s}"
    );
}

#[test]
fn a_box_name_without_the_moor_prefix_keeps_the_lease() {
    for name in ["newman", "moor-", "moo-x"] {
        let h = Host::new();
        let work = h.task("t1", name);
        h.sh(&script("t1", name, &work).unwrap());
        assert_eq!(h.alive("t1", name), [true, false, false], "{name}");
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
        .arg(script("t1", "moor-x", &home.join("repo").display().to_string()).unwrap())
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
        .arg(script("t1", "moor-x", &lease.join("repo").display().to_string()).unwrap())
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
