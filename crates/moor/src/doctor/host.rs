//! The host checks: what a Box start needs from this machine.

use std::fs;
use std::os::unix::fs::MetadataExt;

use moor_client::{kvm, rootless};

use super::{Check, Inputs};
use crate::proxy::forwarder::elf;
use crate::proxy::route;
use crate::up::service;

/// Every host check that applies to this config.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    let mut out = vec![egress(i)];
    out.extend(kvm(i));
    out
}

/// Whether the Boxes of the node `moor up` installed here can open the KVM device every Box
/// gets; none on a host with no such node.
pub(super) fn kvm(i: &Inputs<'_>) -> Option<Check> {
    let dir = service::unit_dir(i.env)?;
    if !dir.join(service::AGENT).exists() {
        return None;
    }
    let device = kvm::device(i.env.get(kvm::DEVICE_VAR));
    Some(
        match kvm::refusal(rootless::detect(), device, kvm::mode_of(device)) {
            Some(f) => Check::fail(f.check, f.detail, f.fix),
            None => Check::ok("kvm", format!("Boxes can open {device}")),
        },
    )
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
