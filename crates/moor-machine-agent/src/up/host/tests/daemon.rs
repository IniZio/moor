//! Setting the rootless daemon up: settings before the setup tool, the context put back, and a
//! daemon that already runs left alone.

use std::path::{Path, PathBuf};

use super::super::{Daemon, Facts};
use super::{ready, run, Fake, SETUPTOOL};

/// The drop-in file of the daemon's unit under `dir`.
fn dropin(dir: &Path) -> PathBuf {
    dir.join("config/systemd/user/docker.service.d/moor.conf")
}

/// A host whose rootless daemon is not set up yet.
fn fresh(dir: &Path) -> Facts {
    let mut f = ready(dir);
    f.daemon = Daemon::Absent;
    f
}

/// A shell whose setup tool switches the CLI to its rootless context, as Docker's does.
fn switching(dir: &Path, ok: bool) -> Fake {
    let config = dir.join("docker/config.json");
    let seen = dir.join("dropin-seen");
    let drop = dropin(dir);
    let mut sh = Fake::new().answer(SETUPTOOL, ok, "[ERROR] setup tool output");
    sh.on_run = Box::new(move |line| {
        if line.starts_with(SETUPTOOL) || line.starts_with("sh -c curl") {
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, r#"{"currentContext":"rootless"}"#).unwrap();
            std::fs::write(&seen, drop.exists().to_string()).unwrap();
        }
    });
    sh
}

/// Checks the drop-in existed when the setup tool ran, and daemon.json kept its other keys.
fn assert_settings_came_first(dir: &Path, json: &Path) {
    assert_eq!(
        std::fs::read_to_string(dir.join("dropin-seen")).unwrap(),
        "true"
    );
    assert_eq!(
        std::fs::read_to_string(dropin(dir)).unwrap(),
        "[Service]\nEnvironment=DOCKERD_ROOTLESS_ROOTLESSKIT_NET=slirp4netns\nEnvironment=PATH=/usr/bin:/usr/sbin:/bin:/sbin\n"
    );
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(json).unwrap()).unwrap();
    assert_eq!(
        kept,
        serde_json::json!({"log-level": "warn", "live-restore": false})
    );
}

#[test]
fn a_fresh_host_gets_rootless_docker_from_the_setup_tool_with_settings_first() {
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("config/docker/daemon.json");
    std::fs::create_dir_all(json.parent().unwrap()).unwrap();
    std::fs::write(&json, r#"{"log-level":"warn","live-restore":true}"#).unwrap();
    let mut sh = switching(dir.path(), true);
    let (result, said, _) = run(&fresh(dir.path()), &fresh(dir.path()), &mut sh, None);
    result.unwrap();
    assert!(
        sh.ran(&format!("{SETUPTOOL} install --force")),
        "{:?}",
        sh.ran
    );
    let env = sh.env_of(SETUPTOOL);
    assert!(
        env.contains(&(
            "PATH".to_owned(),
            Some("/usr/bin:/usr/sbin:/bin:/sbin".to_owned())
        )),
        "{env:?}"
    );
    assert!(env.contains(&("DOCKER_HOST".to_owned(), None)), "{env:?}");
    assert_settings_came_first(dir.path(), &json);
    assert!(sh.ran("docker context use default"), "{:?}", sh.ran);
    assert_eq!(
        sh.env_of("docker context use"),
        [
            ("DOCKER_HOST".to_owned(), None),
            ("DOCKER_CONTEXT".to_owned(), None)
        ]
    );
    assert!(
        sh.ran("systemctl --user enable --now docker"),
        "{:?}",
        sh.ran
    );
    assert!(
        said.ends_with("rootless docker: installed with /usr/bin/dockerd-rootless-setuptool.sh\n"),
        "{said}"
    );
}

#[test]
fn the_context_comes_back_when_the_setup_tool_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut sh = switching(dir.path(), false);
    let (result, _, _) = run(&fresh(dir.path()), &fresh(dir.path()), &mut sh, None);
    let err = result.unwrap_err();
    assert!(err.contains("[ERROR] setup tool output"), "{err}");
    assert!(
        err.contains("then run moor-machine-agent up again"),
        "{err}"
    );
    assert!(sh.ran("docker context use default"), "{:?}", sh.ran);
    assert!(!sh.ran("systemctl --user enable"), "{:?}", sh.ran);
}

#[test]
fn a_context_the_person_chose_is_the_one_restored() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("docker/config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, r#"{"currentContext":"work"}"#).unwrap();
    let mut sh = switching(dir.path(), true);
    let (result, _, _) = run(&fresh(dir.path()), &fresh(dir.path()), &mut sh, None);
    result.unwrap();
    assert!(sh.ran("docker context use work"), "{:?}", sh.ran);
}

#[test]
fn a_host_without_dockers_package_gets_dockers_rootless_installer() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fresh(dir.path());
    f.setuptool = None;
    f.docker_repo = false;
    let mut sh = switching(dir.path(), true);
    let (result, said, _) = run(&f, &f, &mut sh, None);
    result.unwrap();
    assert!(
        sh.ran("sh -c curl -fsSL https://get.docker.com/rootless | sh"),
        "{:?}",
        sh.ran
    );
    let env = sh.env_of("sh -c curl");
    assert!(
        env.contains(&("FORCE_ROOTLESS_INSTALL".to_owned(), Some("1".to_owned()))),
        "{env:?}"
    );
    let bin = dir.path().join("bin").display().to_string();
    assert!(
        std::fs::read_to_string(dropin(dir.path()))
            .unwrap()
            .contains(&format!("Environment=PATH={bin}:/usr/bin:")),
        "PATH pins ~/bin first"
    );
    assert!(sh.ran("docker context use default"), "{:?}", sh.ran);
    assert!(
        said.contains("rootless docker: installed with https://get.docker.com/rootless"),
        "{said}"
    );
}

#[test]
fn a_daemon_json_that_is_not_json_stops_before_the_setup_tool() {
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("config/docker/daemon.json");
    std::fs::create_dir_all(json.parent().unwrap()).unwrap();
    std::fs::write(&json, "{ nope").unwrap();
    let mut sh = Fake::new();
    let (result, _, _) = run(&fresh(dir.path()), &fresh(dir.path()), &mut sh, None);
    let err = result.unwrap_err();
    assert!(err.contains(&json.display().to_string()), "{err}");
    assert!(err.contains("fix it"), "{err}");
    assert!(!sh.ran(SETUPTOOL));
}

#[test]
fn a_stopped_daemon_is_started_and_its_settings_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.daemon = Daemon::Installed { active: false };
    let mut sh = Fake::new();
    let (result, said, _) = run(&f, &f, &mut sh, None);
    result.unwrap();
    assert!(
        sh.ran("systemctl --user enable --now docker"),
        "{:?}",
        sh.ran
    );
    assert!(!sh.ran(SETUPTOOL));
    assert!(!dropin(dir.path()).exists());
    assert!(said.ends_with("rootless docker: started\n"), "{said}");
}

#[test]
fn a_running_unit_whose_daemon_does_not_answer_is_left_for_the_preflight() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = ready(dir.path());
    f.daemon = Daemon::Installed { active: true };
    let mut sh = Fake::new();
    let (result, _, _) = run(&f, &f, &mut sh, None);
    result.unwrap();
    assert!(!sh.ran("systemctl"), "{:?}", sh.ran);
}
