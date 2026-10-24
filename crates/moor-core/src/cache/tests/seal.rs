//! The seal script run under a real `sh` in a temp `HOME`.

use std::path::Path;

use super::*;

/// The file a successful seal leaves in each path of its volume.
const MARKER: &str = ".crabbox-sealed";

/// Every preset but docker, as a drop list.
const ALL_BUT_DOCKER: &str =
    "cache: {presets: [npm, yarn, pnpm, bun, podman, gradle, dart, rust, go]}";

/// A fake `sudo -n` that runs its command with `FAKE_SUDO` set and logs it.
const FAKE_SUDO: &str = "#!/bin/sh\n[ \"$1\" = -n ] && shift\necho \"sudo $*\" >>\"$HOME/log\"\nFAKE_SUDO=1 exec \"$@\"\n";

/// A fake `touch` that only works through [`FAKE_SUDO`], as on a root-owned directory.
const ROOT_ONLY_TOUCH: &str =
    "#!/bin/sh\n[ -n \"$FAKE_SUDO\" ] || exit 1\nfor f in \"$@\"; do : >>\"$f\"; done\n";

/// A fake `rm` that only works through [`FAKE_SUDO`], as on a root-owned marker.
const ROOT_ONLY_RM: &str = "#!/bin/sh\n[ -n \"$FAKE_SUDO\" ] || exit 1\nexec /bin/rm \"$@\"\n";

/// A temp `HOME` with `bin/` holding the host guards, so no `sudo`, `apt-get` or `docker` of
/// this host runs.
fn home() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("moor-seal-")
        .tempdir()
        .unwrap();
    moor_testkit::write_host_guards(&tmp.path().join("bin")).unwrap();
    tmp
}

fn fake(home: &Path, name: &str, body: &str) {
    moor_testkit::write_executable(&home.join("bin").join(name), body).unwrap();
}

/// Runs `script` with `HOME` at `home`; returns stdout and the exit code.
fn run(home: &Path, script: &str) -> (String, i32) {
    let path = format!(
        "{}:{}",
        home.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = std::process::Command::new("sh")
        .args(["-c", script])
        .env("HOME", home)
        .env("PATH", path)
        .output()
        .unwrap();
    (
        String::from_utf8(out.stdout).unwrap(),
        out.status.code().unwrap(),
    )
}

/// Fills `~/<dir>` with a plain file, a dotfile and a nested file.
fn fill(home: &Path, dir: &str) {
    let d = home.join(dir);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    for f in ["a", ".hidden", "sub/b"] {
        std::fs::write(d.join(f), "x").unwrap();
    }
}

fn entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

fn marked(home: &Path, dir: &str) -> bool {
    home.join(dir).join(MARKER).exists()
}

fn sealed_plan(user_yaml: &str) -> CachePlan {
    let l = layers("cache: {presets: false}", user_yaml);
    CachePlan::build(UserId::new(1), Some(&repo("o", "r")), &l, &salt(1)).unwrap()
}

/// The docker preset's seal script, with its `/var/lib/containerd` moved to `~/containerd` so
/// the test never touches the host's.
fn docker_script(home: &Path) -> String {
    let l = layers(ALL_BUT_DOCKER, "");
    let p = CachePlan::build(UserId::new(1), Some(&repo("o", "r")), &l, &salt(1)).unwrap();
    let script = p.seal_script().unwrap();
    assert!(script.contains("'/var/lib/containerd'"), "{script}");
    fill(home, "docker");
    fill(home, "containerd");
    let script = script.replace("'/var/lib/containerd'", "\"$HOME\"/containerd");
    assert!(!script.contains("/var/lib"), "{script}");
    script
}

#[test]
fn a_failing_seal_empties_only_its_own_volume() {
    let h = home();
    for d in ["good", "bad1", "bad2", "plain"] {
        fill(h.path(), d);
    }
    let p = sealed_plan(
        "cache: {volumes: [\
         {name: good, path: ~/good, seal: [\"touch \\\"$HOME/good/sealed\\\"\"]},\
         {name: bad, path: ~/bad1, seal: [\"true\", \"exit 3\", \"touch \\\"$HOME/ran-after\\\"\"]},\
         {name: plain, path: ~/plain}]}",
    );
    let (stdout, code) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!(code, 0, "the seal script never fails the close");
    assert_eq!(seal_failed(&stdout), ["bad"], "{stdout}");
    assert_eq!(
        entries(&h.path().join("bad1")),
        0,
        "the failed volume is emptied"
    );
    assert!(
        h.path().join("bad1").is_dir(),
        "its directory stays, a mount point may sit there"
    );
    assert!(
        !h.path().join("ran-after").exists(),
        "steps stop at the first failure"
    );
    let left = ["good", "plain", "bad2"].map(|d| entries(&h.path().join(d)));
    assert_eq!(
        left,
        [5, 3, 3],
        "the good seal added its file and the marker; nothing else changed"
    );
}

#[test]
fn a_successful_seal_marks_its_volume_and_no_other() {
    let h = home();
    for d in ["one", "two", "bad", "plain"] {
        fill(h.path(), d);
    }
    let p = sealed_plan(
        "cache: {volumes: [\
         {name: good, path: ~/one, seal: [\"true\"]},\
         {name: also, path: ~/two, seal: [\"true\"]},\
         {name: bad, path: ~/bad, seal: [\"false\"]},\
         {name: plain, path: ~/plain}]}",
    );
    let (stdout, _) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!(seal_failed(&stdout), ["bad"], "{stdout}");
    let got = ["one", "two", "bad", "plain"].map(|d| marked(h.path(), d));
    assert_eq!(got, [true, true, false, false]);
}

#[test]
fn the_marker_falls_back_to_sudo() {
    let h = home();
    fill(h.path(), "root-owned");
    fake(h.path(), "sudo", FAKE_SUDO);
    fake(h.path(), "touch", ROOT_ONLY_TOUCH);
    let p = sealed_plan("cache: {volumes: [{name: v, path: ~/root-owned, seal: [\"true\"]}]}");
    let (stdout, _) = run(h.path(), &p.seal_script().unwrap());
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    assert!(marked(h.path(), "root-owned"));
    let log = std::fs::read_to_string(h.path().join("log")).unwrap();
    assert!(log.contains("sudo touch"), "{log}");
}

#[test]
fn a_marker_neither_touch_nor_sudo_can_write_fails_the_seal() {
    let h = home();
    fill(h.path(), "root-owned");
    fake(h.path(), "touch", ROOT_ONLY_TOUCH);
    let p = sealed_plan("cache: {volumes: [{name: v, path: ~/root-owned, seal: [\"true\"]}]}");
    let (stdout, code) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!((seal_failed(&stdout), code), (vec!["v".to_owned()], 0));
    assert!(!marked(h.path(), "root-owned"));
}

#[test]
fn a_missing_directory_is_no_failure() {
    let h = home();
    let p = sealed_plan("cache: {volumes: [{name: bad, path: ~/never, seal: [\"false\"]}]}");
    let (stdout, code) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!((seal_failed(&stdout), code), (vec!["bad".to_owned()], 0));
}

#[test]
fn the_docker_seal_passes_when_docker_is_absent() {
    let h = home();
    let script = docker_script(h.path());
    let tools = h.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    for tool in ["sh", "find", "xargs", "touch", "rm"] {
        let found = ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.exists())
            .unwrap();
        std::os::unix::fs::symlink(found, tools.join(tool)).unwrap();
    }
    std::fs::remove_file(h.path().join("bin/docker")).unwrap();
    let script = format!("PATH=\"$HOME/bin:$HOME/tools\"\n{script}");
    let (stdout, code) = run(h.path(), &script);
    assert_eq!((stdout.as_str(), code), ("", 0));
    assert_eq!(entries(&h.path().join("docker")), 4);
    assert!(marked(h.path(), "docker") && marked(h.path(), "containerd"));
}

#[test]
fn the_docker_seal_uses_docker_directly_when_its_socket_works() {
    let h = home();
    let script = docker_script(h.path());
    fake(
        h.path(),
        "docker",
        "#!/bin/sh\necho \"docker $*\" >>\"$HOME/log\"\n",
    );
    let (stdout, _) = run(h.path(), &script);
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    let log = std::fs::read_to_string(h.path().join("log")).unwrap();
    assert!(log.contains("docker volume prune -af"), "{log}");
    assert!(!log.contains("sudo"), "{log}");
    assert!(marked(h.path(), "docker") && marked(h.path(), "containerd"));
}

#[test]
fn the_docker_seal_falls_back_to_sudo_docker() {
    let h = home();
    let script = docker_script(h.path());
    fake(h.path(), "sudo", FAKE_SUDO);
    fake(
        h.path(),
        "docker",
        "#!/bin/sh\n[ -n \"$FAKE_SUDO\" ] || exit 1\necho \"docker $*\" >>\"$HOME/log\"\n",
    );
    let (stdout, _) = run(h.path(), &script);
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    let log = std::fs::read_to_string(h.path().join("log")).unwrap();
    assert!(log.contains("sudo docker network prune -f"), "{log}");
    assert!(marked(h.path(), "docker") && marked(h.path(), "containerd"));
}

#[test]
fn the_docker_seal_fails_and_leaves_no_marker_when_neither_docker_nor_sudo_works() {
    let h = home();
    let script = docker_script(h.path());
    let (stdout, _) = run(h.path(), &script);
    assert_eq!(seal_failed(&stdout), ["docker"]);
    assert_eq!(entries(&h.path().join("docker")), 0);
    assert!(!marked(h.path(), "docker") && !marked(h.path(), "containerd"));
}

#[test]
fn no_seal_steps_means_no_script() {
    assert_eq!(sealed_plan("").seal_script(), None);
    assert_eq!(CachePlan::none().seal_script(), None);
}

/// Writes a marker from an earlier seal into `~/<dir>`.
fn stale_marker(home: &Path, dir: &str) {
    std::fs::write(home.join(dir).join(MARKER), "old").unwrap();
}

#[test]
fn a_marker_from_an_earlier_seal_is_gone_after_a_failing_seal() {
    let h = home();
    fill(h.path(), "v");
    stale_marker(h.path(), "v");
    fake(h.path(), "find", "#!/bin/sh\nexit 1\n");
    let p = sealed_plan("cache: {volumes: [{name: v, path: ~/v, seal: [\"false\"]}]}");
    let (stdout, code) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!((seal_failed(&stdout), code), (vec!["v".to_owned()], 0));
    assert!(
        !marked(h.path(), "v"),
        "the emptying failed, the marker went first"
    );
}

#[test]
fn every_marker_is_removed_before_the_first_seal_step() {
    let h = home();
    for d in ["one", "two"] {
        fill(h.path(), d);
        stale_marker(h.path(), d);
    }
    let p = sealed_plan(
        "cache: {volumes: [\
         {name: one, path: ~/one, seal: [\"[ ! -e \\\"$HOME/one/.crabbox-sealed\\\" ] && [ ! -e \\\"$HOME/two/.crabbox-sealed\\\" ] && touch \\\"$HOME/saw-clean\\\"\"]},\
         {name: two, path: ~/two, seal: [\"true\"]}]}",
    );
    let (stdout, _) = run(h.path(), &p.seal_script().unwrap());
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    assert!(h.path().join("saw-clean").exists());
}

#[test]
fn a_successful_seal_leaves_one_fresh_marker_per_path() {
    let h = home();
    let script = docker_script(h.path());
    for d in ["docker", "containerd"] {
        stale_marker(h.path(), d);
    }
    fake(h.path(), "docker", "#!/bin/sh\n");
    let (stdout, _) = run(h.path(), &script);
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    for d in ["docker", "containerd"] {
        assert!(marked(h.path(), d), "{d}");
        let marker = std::fs::read(h.path().join(d).join(MARKER)).unwrap();
        assert!(marker.is_empty(), "{d} holds the old marker");
        assert_eq!(
            entries(&h.path().join(d)),
            4,
            "{d}: its files and one marker"
        );
    }
}

#[test]
fn the_marker_removal_falls_back_to_sudo() {
    let h = home();
    fill(h.path(), "root-owned");
    stale_marker(h.path(), "root-owned");
    fake(h.path(), "sudo", FAKE_SUDO);
    fake(h.path(), "rm", ROOT_ONLY_RM);
    let p = sealed_plan(
        "cache: {volumes: [{name: v, path: ~/root-owned, seal: [\"[ ! -e \\\"$HOME/root-owned/.crabbox-sealed\\\" ]\"]}]}",
    );
    let (stdout, _) = run(h.path(), &p.seal_script().unwrap());
    assert!(seal_failed(&stdout).is_empty(), "{stdout}");
    let log = std::fs::read_to_string(h.path().join("log")).unwrap();
    assert!(log.contains("sudo rm -f"), "{log}");
    assert!(marked(h.path(), "root-owned"), "the fresh marker");
}

#[test]
fn a_marker_neither_rm_nor_sudo_can_remove_fails_the_seal_before_its_steps() {
    let h = home();
    fill(h.path(), "root-owned");
    stale_marker(h.path(), "root-owned");
    fake(h.path(), "rm", ROOT_ONLY_RM);
    let p = sealed_plan(
        "cache: {volumes: [{name: v, path: ~/root-owned, seal: [\"touch \\\"$HOME/ran\\\"\"]}]}",
    );
    let (stdout, code) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!((seal_failed(&stdout), code), (vec!["v".to_owned()], 0));
    assert!(
        !h.path().join("ran").exists(),
        "no step runs over a stale marker"
    );
}
