//! The restore script run under a real `sh` in a temp `HOME`.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;

const ALL_BUT_DOCKER: &str =
    "cache: {presets: [cache, pnpm, bun, podman, gradle, dart, rust, go, maven, cocoapods, bundler, android, android-sdk], work_tree: false}";

const FAKE_SUDO: &str = "#!/bin/sh\n[ \"$1\" = -n ] && shift\nFAKE_SUDO=1 exec \"$@\"\n";

const DOCKER_UP: &str = "#!/bin/sh\n[ \"$1\" = is-active ] && exit 0\necho \"$1 $(ls \"$HOME/containerd\" | wc -l)\" >>\"$HOME/log\"\n";

const DOCKER_ABSENT: &str = "#!/bin/sh\necho \"$1\" >>\"$HOME/log\"\nexit 3\n";

const CONTENT: &str = "containerd/io.containerd.content.v1.content";

fn home() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("moor-restore-")
        .tempdir()
        .unwrap();
    moor_testkit::write_host_guards(&tmp.path().join("bin")).unwrap();
    fake(tmp.path(), "systemctl", DOCKER_ABSENT);
    tmp
}

fn fake(home: &Path, name: &str, body: &str) {
    moor_testkit::write_executable(&home.join("bin").join(name), body).unwrap();
}

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

fn put(home: &Path, rel: &str, content: &str) {
    let f = home.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, content).unwrap();
}

fn entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

fn log(home: &Path) -> String {
    std::fs::read_to_string(home.join("log")).unwrap_or_default()
}

fn fill_docker(home: &Path) {
    let name = hex(&Sha256::digest(b"layer"));
    put(home, &format!("{CONTENT}/blobs/sha256/{name}"), "layer");
    put(home, "containerd/snapshots/1/fs", "image");
    put(home, "docker/buildkit/cache", "build");
    put(home, &format!("{CONTENT}/ingest/1/data"), "partial");
    for left in ["containers/c1/config", "volumes/v1/data", "network/kv.db"] {
        put(home, &format!("docker/{left}"), "old");
    }
}

fn corrupt_a_blob(home: &Path) {
    put(
        home,
        &format!("{CONTENT}/blobs/sha256/{}", "0".repeat(64)),
        "x",
    );
}

fn docker_script() -> String {
    let l = layers(ALL_BUT_DOCKER, "");
    let script = plan(&l).restore_script().unwrap();
    assert!(script.contains("'/var/lib/containerd'"), "{script}");
    let script = script
        .replace("'/var/lib/containerd'", "\"$HOME\"/containerd")
        .replace("/var/lib/containerd", "$HOME/containerd");
    assert!(!script.contains("/var/lib"), "{script}");
    script
}

fn declared(yaml: &str) -> CachePlan {
    plan(&layers("cache: {presets: false, work_tree: false}", yaml))
}

#[test]
fn a_bad_blob_empties_both_docker_paths_and_prints_the_line() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fill_docker(h.path());
    corrupt_a_blob(h.path());
    let (stdout, code) = run(h.path(), &docker_script());
    assert_eq!(code, 0, "a bad cache never fails the start");
    let [e]: [Emptied; 1] = emptied(&stdout).try_into().unwrap();
    assert_eq!(e.volume, "docker");
    assert_eq!(
        e.reason,
        format!("blob {} does not match its hash", "0".repeat(64))
    );
    assert!(stdout.starts_with(EMPTIED), "{stdout}");
    assert_eq!(entries(&h.path().join("docker")), 0);
    assert_eq!(entries(&h.path().join("containerd")), 0);
}

#[test]
fn a_good_blob_keeps_images_and_removes_what_the_last_box_left() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fill_docker(h.path());
    let (stdout, code) = run(h.path(), &docker_script());
    assert_eq!((stdout.as_str(), code), ("", 0));
    let p = h.path();
    for gone in ["containers", "volumes", "network"] {
        assert!(!p.join("docker").join(gone).exists(), "{gone}");
    }
    assert!(!p.join(CONTENT).join("ingest").exists());
    assert_eq!(entries(&p.join(CONTENT).join("blobs/sha256")), 1);
    assert!(p.join("containerd/snapshots/1/fs").exists());
    assert!(p.join("docker/buildkit/cache").exists());
}

#[test]
fn a_box_with_no_sudo_cleans_what_its_user_owns() {
    let h = home();
    fill_docker(h.path());
    let (stdout, code) = run(h.path(), &docker_script());
    assert_eq!((stdout.as_str(), code), ("", 0));
    assert!(!h.path().join("docker/containers").exists());
}

#[test]
fn a_rerun_with_the_stamp_does_nothing() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fill_docker(h.path());
    let script = docker_script();
    assert_eq!(run(h.path(), &script), (String::new(), 0));
    put(h.path(), "docker/containers/live/config", "live");
    corrupt_a_blob(h.path());
    assert_eq!(run(h.path(), &script), (String::new(), 0));
    assert!(h.path().join("docker/containers/live/config").exists());
    assert!(h
        .path()
        .join(CONTENT)
        .join("blobs/sha256")
        .join("0".repeat(64))
        .exists());
}

#[test]
fn the_stamp_is_written_only_after_every_step_ran() {
    let h = home();
    put(h.path(), "v/x", "x");
    let p = declared(
        "cache: {volumes: [{name: v, path: ~/v, restore: [\"[ ! -e \\\"$HOME/.moor/cache-restored\\\" ]\"]}]}",
    );
    assert_eq!(
        run(h.path(), &p.restore_script().unwrap()),
        (String::new(), 0)
    );
    assert!(h.path().join(".moor/cache-restored").exists());
}

#[test]
fn a_failed_agent_check_empties_only_that_volume() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fill_docker(h.path());
    put(h.path(), "agent/versions/1", "partial");
    put(h.path(), "tree/keep", "x");
    let l = CacheLayers {
        agent: Some(AgentCache {
            dirs: vec!["~/agent".to_owned()],
            check: "echo the install is partial; exit 1".to_owned(),
        }),
        ..layers(ALL_BUT_DOCKER, "")
    };
    let script = plan(&l)
        .restore_script()
        .unwrap()
        .replace("'/var/lib/containerd'", "\"$HOME\"/containerd")
        .replace("/var/lib/containerd", "$HOME/containerd");
    let (stdout, code) = run(h.path(), &script);
    assert_eq!(code, 0);
    let [e]: [Emptied; 1] = emptied(&stdout).try_into().unwrap();
    assert_eq!(e.volume, "agent");
    assert_eq!(e.reason, "the install is partial");
    assert_eq!(entries(&h.path().join("agent")), 0);
    assert!(h.path().join("containerd/snapshots/1/fs").exists());
    assert!(h.path().join("tree/keep").exists());
}

#[test]
fn a_failed_step_stops_the_next_and_says_a_default_reason_when_silent() {
    let h = home();
    put(h.path(), "v/x", "x");
    let p = declared(
        "cache: {volumes: [{name: v, path: ~/v, restore: [\"exit 3\", \"touch \\\"$HOME/ran\\\"\"]}]}",
    );
    let (stdout, code) = run(h.path(), &p.restore_script().unwrap());
    assert_eq!(code, 0);
    assert_eq!(stdout, "moor: cache emptied v: a restore step failed\n");
    assert!(!h.path().join("ran").exists());
}

#[test]
fn a_volume_holding_nothing_is_skipped() {
    let h = home();
    std::fs::create_dir(h.path().join("v")).unwrap();
    let p = declared("cache: {volumes: [{name: v, path: ~/v, restore: [exit 3]}]}");
    assert_eq!(
        run(h.path(), &p.restore_script().unwrap()),
        (String::new(), 0)
    );
}

#[test]
fn a_volume_its_user_cannot_read_still_runs_its_steps() {
    use std::os::unix::fs::PermissionsExt;
    let h = home();
    let v = h.path().join("v");
    std::fs::create_dir(&v).unwrap();
    std::fs::set_permissions(&v, std::fs::Permissions::from_mode(0o000)).unwrap();
    let p = declared(
        "cache: {volumes: [{name: v, path: ~/v, restore: [\"touch \\\"$HOME/ran\\\"\"]}]}",
    );
    run(h.path(), &p.restore_script().unwrap());
    std::fs::set_permissions(&v, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(h.path().join("ran").exists());
}

#[test]
fn a_clear_that_fails_says_partial_contents_may_remain() {
    let h = home();
    put(h.path(), "v/x", "x");
    let real = run(h.path(), "command -v find").0;
    fake(
        h.path(),
        "find",
        &format!(
            "#!/bin/sh\ncase \"$*\" in *-delete*) exit 1;; esac\nexec {} \"$@\"\n",
            real.trim()
        ),
    );
    let p = declared("cache: {volumes: [{name: v, path: ~/v, restore: [exit 3]}]}");
    let (stdout, _) = run(h.path(), &p.restore_script().unwrap());
    assert_eq!(
        stdout,
        "moor: cache emptied v: a restore step failed; partial contents may remain\n"
    );
}

#[test]
fn a_running_docker_is_stopped_first_and_started_after_a_clean() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fake(h.path(), "systemctl", DOCKER_UP);
    fill_docker(h.path());
    assert_eq!(run(h.path(), &docker_script()), (String::new(), 0));
    assert_eq!(log(h.path()), "stop 2\nreset-failed 2\nstart 2\n");
}

#[test]
fn a_running_docker_starts_again_only_after_a_bad_cache_is_emptied() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fake(h.path(), "systemctl", DOCKER_UP);
    fill_docker(h.path());
    corrupt_a_blob(h.path());
    let (stdout, _) = run(h.path(), &docker_script());
    assert_eq!(emptied(&stdout).len(), 1, "{stdout}");
    assert_eq!(log(h.path()), "stop 2\nreset-failed 0\nstart 0\n");
}

#[test]
fn a_docker_that_was_not_running_is_left_alone() {
    let h = home();
    fake(h.path(), "sudo", FAKE_SUDO);
    fill_docker(h.path());
    run(h.path(), &docker_script());
    assert_eq!(log(h.path()), "is-active\n");
}

#[test]
fn no_volume_with_steps_means_no_script() {
    let p = plan(&layers("cache: {presets: false}", ""));
    assert_eq!(names(&p), ["worktree"]);
    assert_eq!(p.restore_script(), None);
}

#[test]
fn the_seal_key_is_refused_with_no_alias() {
    let yaml = "cache: {volumes: [{name: v, path: ~/v, seal: [\"rm -rf x\"]}]}";
    let repo = parse_repo(".moor.yaml", yaml).unwrap_err().to_string();
    assert!(
        repo.contains("unknown field") && repo.contains("seal"),
        "{repo}"
    );
    let user = parse_user("config.yaml", yaml).unwrap_err().to_string();
    assert!(
        user.contains("unknown field") && user.contains("seal"),
        "{user}"
    );
}

#[test]
fn a_blank_restore_step_is_refused() {
    let yaml = "cache: {volumes: [{name: v, path: ~/v, restore: [\" \"]}]}";
    let e = parse_repo(".moor.yaml", yaml).unwrap_err().to_string();
    assert!(e.contains("restore"), "{e}");
}

#[test]
fn only_a_valid_name_reads_as_emptied() {
    let got = emptied("moor: cache emptied docker: why: more\nmoor: cache emptied BAD: x\nnoise\n");
    assert_eq!(
        got,
        [Emptied {
            volume: "docker".to_owned(),
            reason: "why: more".to_owned()
        }]
    );
}
