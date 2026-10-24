//! The seal script run under a real `sh` in a temp `HOME`.

use std::path::Path;

use super::*;

/// A temp `HOME` with `bin/` holding the host guards, so no `sudo` or `apt-get` of this host runs.
fn home() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("moor-seal-")
        .tempdir()
        .unwrap();
    moor_testkit::write_host_guards(&tmp.path().join("bin")).unwrap();
    tmp
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

fn sealed_plan(user_yaml: &str) -> CachePlan {
    let l = layers("cache: {presets: false}", user_yaml);
    CachePlan::build(UserId::new(1), Some(&repo("o", "r")), &l, &salt(1)).unwrap()
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
        [4, 3, 3],
        "the good seal added its marker; nothing else changed"
    );
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
    fill(h.path(), "docker");
    let l = layers("cache: {presets: [npm]}", "");
    let p = CachePlan::build(UserId::new(1), Some(&repo("o", "r")), &l, &salt(1)).unwrap();
    let tools = h.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    for tool in ["sh", "find", "xargs"] {
        let found = ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.exists())
            .unwrap();
        std::os::unix::fs::symlink(found, tools.join(tool)).unwrap();
    }
    let script = format!(
        "PATH=\"$HOME/bin:$HOME/tools\"\n{}",
        p.seal_script().unwrap()
    );
    let (stdout, code) = run(h.path(), &script);
    assert_eq!((stdout.as_str(), code), ("", 0));
    assert_eq!(entries(&h.path().join("docker")), 3);
}

#[test]
fn the_docker_seal_clears_the_data_root_when_docker_fails() {
    let h = home();
    fill(h.path(), "docker");
    moor_testkit::write_executable(&h.path().join("bin/docker"), "#!/bin/sh\nexit 1\n").unwrap();
    let p = plan(&layers("", ""));
    let (stdout, _) = run(h.path(), &p.seal_script().unwrap());
    assert_eq!(seal_failed(&stdout), ["docker"]);
    assert_eq!(entries(&h.path().join("docker")), 0);
}

#[test]
fn no_seal_steps_means_no_script() {
    assert_eq!(sealed_plan("").seal_script(), None);
    assert_eq!(CachePlan::none().seal_script(), None);
}
