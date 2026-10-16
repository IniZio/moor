use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use super::{box_name, script, script_with};
use crate::adapters::box_agent::device::{self, DeviceKind};
use moor_core::name::BoxPath;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::LaunchReq;
use moor_core::BoxHandle;

/// A device kind that only has a release line.
struct Fake(&'static str);

impl DeviceKind for Fake {
    fn available(&self, _os: &str) -> bool {
        true
    }
    fn provision(&self, _: &dyn Driver, _: &LaunchReq, _: &BoxPath) -> Result<String, String> {
        Err(String::new())
    }
    fn env(&self, _: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    fn grants(&self, _: &str, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn brief(&self, _: &str) -> String {
        String::new()
    }
    fn proof(&self, _: &str) -> String {
        String::new()
    }
    fn teardown(&self, _: &str) -> Option<String> {
        None
    }
    fn release_line(&self, task: &str) -> String {
        format!("echo {}-{task}", self.0)
    }
}

static A: Fake = Fake("a");
static B: Fake = Fake("b");

#[test]
fn every_given_kinds_release_line_is_run_in_order() {
    let w = "/r/moor-a-x/repo";
    let s = script_with("t", "7", "moor-a-x", w, &[&A, &B]).unwrap();
    let (a, b) = (s.find("echo a-7"), s.find("echo b-7"));
    assert!(a.is_some() && b.is_some() && a < b, "{s}");
    let none = script_with("t", "7", "moor-a-x", w, &[]).unwrap();
    assert!(
        !none.contains("echo a-7") && !none.contains("echo b-7"),
        "{none}"
    );
}

#[test]
fn script_uses_the_enabled_kinds() {
    let w = "/r/moor-a-x/repo";
    assert_eq!(
        script("t", "7", "moor-a-x", w),
        script_with("t", "7", "moor-a-x", w, device::ENABLED)
    );
}

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
    h.sh(&script("t1", "1", "moor-a-x", &a).unwrap());
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
    h.sh(&script("a", "1", "moor-a-x", &short).unwrap());
    assert_eq!(h.alive("a", "moor-a-x"), [false; 3]);
    assert_eq!(h.alive("a-b", "moor-b-y"), [true; 3]);
    h.sh(&script("a-b", "1", "moor-b-y", &long).unwrap());
    assert_eq!(h.alive("a-b", "moor-b-y"), [false; 3]);
}

#[test]
fn the_script_names_the_task_not_an_id() {
    let s = script("fix-it", "1", "moor-a-x", "/r/moor-a-x/repo").unwrap();
    assert!(s.contains(".moor-cred-fix-it-[0-9a-f]"), "{s}");
    assert!(s.contains(".moor-tmp-fix-it-[0-9a-f]"), "{s}");
    assert!(!s.contains("-*"), "{s}");
}

#[test]
fn the_script_kills_only_exact_path_matches() {
    let s = script("t", "1", "moor-a-x", "/r/moor-a-x/repo").unwrap();
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
    h.sh(&script("t", "1", "moor-a-x", &w).unwrap());
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
    h.sh(&script("t1", "1", "moor-other-x", &w).unwrap());
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
        let s = script("t1", "1", "moor-a-x", w).unwrap();
        assert_eq!(s.matches("esac").count(), 1, "{w}: {s}");
    }
}

#[test]
fn refuses_unsafe_input() {
    for id in [
        "", ".", "..", "t 1", "t/1", "t*", "t;rm", "$HOME", "t\n", "'",
    ] {
        assert!(
            script(id, "1", "moor-a-x", "/r/moor-a-x/repo").is_none(),
            "{id:?}"
        );
        assert!(
            script("t1", "1", id, "/r/moor-a-x/repo").is_none(),
            "{id:?}"
        );
    }
    assert!(script("t1", "1", "moor-a-x", "r/moor-a-x/repo").is_none());
    assert!(script("t1", "1", "moor-a-x", "").is_none());
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
        assert!(
            script(n, "1", "moor-a-x", "/r/moor-a-x/repo").is_none(),
            "{n:?}"
        );
    }
    for n in ["a", "0", "a-b", "07", &"a".repeat(40)] {
        assert!(
            script(n, "1", "moor-a-x", "/r/moor-a-x/repo").is_some(),
            "{n:?}"
        );
    }
}

#[test]
fn quotes_a_workdir_with_odd_characters() {
    let h = Host::new();
    let work = h.root().join("moor-a-x").join("it's a repo");
    fs::create_dir_all(&work).unwrap();
    h.sh(&script("t1", "1", "moor-a-x", &work.display().to_string()).unwrap());
    assert!(!h.root().join("moor-a-x").exists());
}

#[test]
fn an_empty_home_removes_nothing() {
    let h = Host::new();
    let w = h.task("t1", "moor-a-x");
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("t1", "1", "moor-a-x", &w).unwrap())
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
    let s = script("t7", "7", "moor-t-n1", "/srv/crabbox/moor-t-n1/repo").unwrap();
    assert!(
        s.starts_with(
            "[ -n \"$HOME\" ] && [ \"$HOME\" != / ] || exit 1
"
        ),
        "{s}"
    );
    assert!(
        s.contains("
case \"$HOME/\" in '/srv/crabbox/moor-t-n1/repo.moor-home'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1/repo.moor-home' ;; esac
"),
        "{s}"
    );
    let lease = "case \"$HOME/\" in '/srv/crabbox/moor-t-n1'/*) ;; *) rm -rf -- '/srv/crabbox/moor-t-n1' ;; esac
";
    let (_, after) = s.split_once(lease).expect("lease removal");
    assert!(after.contains("moor-7 ("), "{s}");
}

const LISTING: &str = "\
== Devices ==
-- iOS 26.0 --
    iPhone 17 Pro (11111111-1111-1111-1111-111111111111) (Shutdown)
    fc791-s6 (22222222-2222-2222-2222-222222222222) (Booted)
    moor-421 (33333333-3333-3333-3333-333333333333) (Shutdown)
    xmoor-42 (44444444-4444-4444-4444-444444444444) (Shutdown)
    moor-42 (55555555-5555-5555-5555-555555555555) (Booted)
";

/// Runs the script for task 42 with a fake `xcrun`; returns what it was asked
/// to do, one call per line.
fn xcrun_calls(simctl_fails: bool) -> String {
    let h = Host::new();
    let bin = h.dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = h.dir.path().join("calls");
    let fake = format!(
        "#!/bin/sh\necho \"$*\" >> '{}'\n\
         if [ \"$2\" = list ]; then cat <<'EOF'\n{LISTING}EOF\nfi\n\
         [ \"{simctl_fails}\" = false ]\n",
        log.display()
    );
    let path = bin.join("xcrun");
    fs::write(&path, fake).unwrap();
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let w = h.task("42", "moor-a-x");
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("42", "42", "moor-a-x", &w).unwrap())
        .env("HOME", h.home())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(h.alive("42", "moor-a-x"), [false; 3]);
    fs::read_to_string(log).unwrap()
}

#[test]
fn deletes_only_the_tasks_exactly_named_simulator() {
    let calls = xcrun_calls(false);
    assert!(calls.contains("simctl delete 55555555-5555-5555-5555-555555555555"));
    assert_eq!(calls.matches("delete").count(), 1, "{calls}");
    assert!(
        !calls.contains("booted") && !calls.contains("all"),
        "{calls}"
    );
}

#[test]
fn the_device_release_line_uses_the_id_not_the_name() {
    let s = script("fix-login", "42", "moor-a-x", "/r/moor-a-x/repo").unwrap();
    assert!(s.contains("moor-42 ("), "{s}");
    assert!(!s.contains("moor-fix-login"), "{s}");
    assert!(s.contains(".moor-cred-fix-login-[0-9a-f]"), "{s}");
}

#[test]
fn refuses_a_task_id_that_is_not_digits() {
    for id in [
        "",
        "fix-login",
        "4 2",
        "42;rm",
        "$HOME",
        "42\n",
        "-1",
        "\u{664}",
    ] {
        assert!(
            script("t1", id, "moor-a-x", "/r/moor-a-x/repo").is_none(),
            "{id:?}"
        );
    }
}

#[test]
fn a_failing_simctl_does_not_fail_the_cleanup() {
    let calls = xcrun_calls(true);
    assert!(calls.contains("simctl delete 55555555"), "{calls}");
}

#[test]
fn a_host_without_xcrun_is_not_a_failure() {
    let h = Host::new();
    let w = h.task("42", "moor-a-x");
    let st = Command::new("sh")
        .arg("-c")
        .arg(script("42", "42", "moor-a-x", &w).unwrap())
        .env("HOME", h.home())
        .env("PATH", "/usr/bin:/bin")
        .status()
        .unwrap();
    assert!(st.success());
}

#[test]
fn a_box_name_without_the_moor_prefix_keeps_the_lease() {
    for name in ["newman", "moor-", "moo-x"] {
        let h = Host::new();
        let work = h.task("t1", name);
        h.sh(&script("t1", "1", name, &work).unwrap());
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
        .arg(
            script(
                "t1",
                "1",
                "moor-x",
                &home.join("repo").display().to_string(),
            )
            .unwrap(),
        )
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
        .arg(
            script(
                "t1",
                "1",
                "moor-x",
                &lease.join("repo").display().to_string(),
            )
            .unwrap(),
        )
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
    h.sh(&script("1", "1", "moor-other-x", &w).unwrap());
    assert!(!Path::new(&agent_home).exists());
    assert!(Path::new(&w).is_dir());
}

#[test]
fn removes_the_per_task_home_with_the_lease() {
    let h = Host::new();
    let w = h.task("1", "moor-a-x");
    fs::create_dir_all(format!("{w}.moor-home")).unwrap();
    h.sh(&script("1", "1", "moor-a-x", &w).unwrap());
    assert_eq!(h.alive("1", "moor-a-x"), [false; 3]);
}

#[test]
fn a_relative_or_empty_workdir_emits_no_home_removal() {
    for w in ["", "repo", "r/moor-a-x/repo"] {
        assert!(script("1", "1", "moor-a-x", w).is_none(), "{w:?}");
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
            .arg(script("1", "1", "moor-x", &work.display().to_string()).unwrap())
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(st.success());
        assert!(home.join("keep").exists(), "{sub:?}");
    }
}
