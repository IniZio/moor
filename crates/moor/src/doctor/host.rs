//! The host checks: what a Box start needs from this machine.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};

use moor_client::{kvm, rootless};

use super::{images, Check, Inputs};
use crate::proxy::forwarder::elf;
use crate::proxy::route;
use crate::up::service;

/// Every host check that applies to this config.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    let mut out = vec![egress(i)];
    out.extend(kvm(i));
    out
}

/// Whether the node `moor up` installed here runs on rootless Docker and its Boxes can open the
/// KVM device every Box gets; none on a host with no such node.
fn kvm(i: &Inputs<'_>) -> Option<Check> {
    kvm_with(i, &daemon_is_rootless)
}

/// Whether the daemon some `docker` variables name is rootless Docker; none when it does not answer.
pub(super) type Rootless = dyn Fn(&[(String, String)]) -> Option<bool>;

/// [`kvm()`], where `rootless` says whether the daemon that the agent unit's `docker` variables
/// name is rootless Docker.
pub(super) fn kvm_with(i: &Inputs<'_>, rootless: &Rootless) -> Option<Check> {
    let unit = fs::read_to_string(service::unit_dir(i.env)?.join(service::AGENT)).ok()?;
    let docker_env: Vec<(String, String)> = ["DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG"]
        .iter()
        .filter_map(|n| Some(((*n).to_owned(), images::unit_var(&unit, n)?)))
        .collect();
    if rootless(&docker_env) == Some(false) {
        let f = rootless::refusal();
        return Some(Check::fail(f.check, f.detail, f.fix));
    }
    let device = kvm::device(i.env.get(kvm::DEVICE_VAR));
    Some(match kvm::refusal(device, kvm::mode_of(device)) {
        Some(f) => Check::fail(f.check, f.detail, f.fix),
        None => Check::ok("kvm", format!("Boxes can open {device}")),
    })
}

/// Whether the daemon `docker_env` points `docker` at is rootless Docker; none when it does not
/// answer.
fn daemon_is_rootless(docker_env: &[(String, String)]) -> Option<bool> {
    Command::new("docker")
        .args(rootless::SECURITY_ARGS)
        .envs(docker_env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| rootless::is_rootless(&String::from_utf8_lossy(&o.stdout)))
}

/// The forwarder binary the controller delivers to each Box, when the Proxy runs on this host.
pub(super) fn egress(i: &Inputs<'_>) -> Check {
    if !route::is_local(i.env) {
        return Check::ok(
            "egress",
            "Boxes reach a remote egress Proxy; no forwarder needed",
        );
    }
    let bin = route::binary(i.env);
    let runnable = fs::metadata(&bin).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0);
    let loader = runnable
        .then(|| fs::read(&bin).ok())
        .flatten()
        .and_then(|bytes| elf::interpreter(&bytes));
    if let Some(loader) = loader {
        return Check::fail("egress", elf::dynamic(&bin, &loader), elf::STATIC);
    }
    if runnable {
        return Check::ok(
            "egress",
            format!("moor-egress-forward at {}", bin.display()),
        );
    }
    Check::fail(
        "egress",
        format!("moor-egress-forward not found at {}", bin.display()),
        "install it beside moor, or set MOOR_EGRESS_FORWARD_BIN to it",
    )
}
