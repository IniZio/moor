//! `moor up` on a host it refuses: rootful Docker, rootless Docker that is not ready, and Boxes
//! that cannot open KVM. It writes nothing there.

use moor_testkit::write_executable;

use super::up::{Host, VERSION};

/// The `docker` answer of a rootless daemon whose rootlesskit `up` cannot read, because the
/// daemon is not at a Unix socket on this host; it comes before the ready daemon's own.
const NOT_LOCAL: &str =
    "  \"context inspect --format {{.Endpoints.docker.Host}}\") echo tcp://10.0.0.2:2375 ;;\n";

#[test]
fn up_on_rootless_docker_that_is_not_ready_names_each_fix_and_writes_nothing() {
    let host = Host::new();
    host.docker(&format!("{VERSION}{NOT_LOCAL}"));
    let run = host.moor(&["up"], "");
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(
        run.stdout
            .contains("rootless Docker is not ready for Boxes:"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("- loopback: "), "{}", run.stdout);
    assert!(run.stdout.contains("; fix: "), "{}", run.stdout);
    assert!(
        !run.stdout
            .contains("Docker Boxes run on rootless Docker as"),
        "{}",
        run.stdout
    );
    assert!(host.written().iter().all(|(_, text)| text.is_empty()));
    assert_eq!(
        host.systemctl_log(),
        "--user is-enabled docker\n",
        "only a read"
    );
}

#[test]
fn up_on_rootful_docker_refuses_with_the_runbook_and_writes_nothing() {
    let host = Host::new();
    write_executable(
        &host.bin.join("docker"),
        format!("#!/bin/sh\ncase \"$*\" in\n{VERSION}  *) echo '[]' ;;\nesac\n"),
    )
    .unwrap();
    let run = host.moor(&["up"], "");
    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stdout.starts_with(
            "fail  docker      this Docker daemon is not rootless, and Docker Boxes run only on rootless Docker; fix: "
        ),
        "{run:?}"
    );
    assert!(
        run.stdout
            .contains("export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/docker.sock"),
        "{run:?}"
    );
    assert!(run.stdout.contains("docs/rootless-docker.md"), "{run:?}");
    assert!(host.written().iter().all(|(_, text)| text.is_empty()));
    assert_eq!(host.systemctl_log(), "");
}

#[test]
fn up_on_a_host_whose_boxes_cannot_open_kvm_refuses_and_writes_nothing() {
    let host = Host::new();
    let missing = host.bin.join("no-kvm");
    let r = host.moor_with(
        &["up"],
        "",
        &[("MOOR_KVM_DEVICE", missing.to_str().unwrap())],
    );
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.stdout.starts_with(&format!(
            "fail  kvm         {} is missing, and every Box gets KVM; fix: ",
            missing.display()
        )),
        "{r:?}"
    );
    assert!(r.stdout.contains("docs/rootless-docker.md#kvm"), "{r:?}");
    assert!(!host.units().exists());
    assert_eq!(host.systemctl_log(), "");
}
