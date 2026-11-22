//! What `up` does to the host before it pairs: the sudo command it asks to run, the docker it
//! points the service at, and its refusals of a daemon that is not ready.

use std::os::unix::fs::PermissionsExt;

use super::{docker, world};

/// A `docker` of rootless Docker whose rootlesskit `up` cannot read, because the daemon is
/// not at a Unix socket on this host.
const ROOTLESS_DOCKER: &str = r#"#!/bin/sh
case "$*" in
  "info --format {{json .SecurityOptions}}") echo '["name=seccomp,profile=builtin","name=rootless"]' ;;
  "context inspect --format {{.Endpoints.docker.Host}}") echo tcp://10.0.0.2:2375 ;;
  *) echo 27.0.1 ;;
esac
"#;

#[test]
fn up_on_rootless_docker_that_is_not_ready_says_each_fix_before_pairing() {
    let w = world();
    let docker = w.home.path().join("bin/docker");
    std::fs::write(&docker, ROOTLESS_DOCKER).unwrap();
    let ran = w.up();
    assert!(!ran.ok);
    assert!(
        ran.said.contains("rootless Docker is not ready"),
        "{}",
        ran.said
    );
    assert!(ran.said.contains("- loopback:"), "{}", ran.said);
    assert!(ran.said.contains("fix: "), "{}", ran.said);
    assert!(
        !ran.said.contains("Docker Boxes run on rootless Docker as"),
        "{}",
        ran.said
    );
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert!(!w.unit().exists());
}

#[test]
fn up_on_rootful_docker_refuses_with_the_runbook_before_pairing() {
    let w = world();
    moor_testkit::write_executable(&w.home.path().join("bin/docker"), docker("")).unwrap();
    let ran = w.up();
    assert!(!ran.ok);
    assert!(
        ran.said.contains(
            "docker: this Docker daemon is not rootless, and Docker Boxes run only on rootless Docker; fix: "
        ),
        "{}",
        ran.said
    );
    assert!(
        ran.said
            .contains("export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock"),
        "{}",
        ran.said
    );
    assert!(ran.said.contains("docs/rootless-docker.md"), "{}", ran.said);
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert!(!w.unit().exists());
}

#[test]
fn up_points_the_service_at_the_rootless_daemon_with_nothing_exported() {
    let w = world();
    let ran = w.up();
    assert!(ran.ok, "{}", ran.said);
    assert!(ran.said.contains("host: ok\n"), "{}", ran.said);
    assert!(ran.said.contains("rootless docker: ok\n"), "{}", ran.said);
    let unit = std::fs::read_to_string(w.unit()).unwrap();
    let host = format!(
        "DOCKER_HOST=unix://{}/docker.sock",
        w.home.path().join("run").display()
    );
    assert!(unit.contains(&host), "{unit}");
}

#[test]
fn up_without_a_terminal_prints_the_sudo_command_and_stops_before_pairing() {
    let w = world();
    let kvm = w.home.path().join("kvm");
    std::fs::write(&kvm, "").unwrap();
    std::fs::set_permissions(&kvm, std::fs::Permissions::from_mode(0o660)).unwrap();
    let ran = w.up();
    assert!(!ran.ok);
    assert!(
        ran.said.contains("this host needs root for:"),
        "{}",
        ran.said
    );
    assert!(ran.said.contains("\nsudo sh -ec '"), "{}", ran.said);
    assert!(ran.said.contains("65-kvm.rules"), "{}", ran.said);
    assert!(
        ran.said
            .contains("run the sudo command above, then run moor-machine-agent up again"),
        "{}",
        ran.said
    );
    assert!(w.log("sudo.log").is_empty(), "sudo never runs unasked");
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert!(!w.unit().exists());
}
