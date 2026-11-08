//! The host checks: what a Box start needs from this machine beyond crabbox itself.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{Check, Inputs};
use crate::proxy::route;
use crate::wire::keepers_tunnel;

/// The runtime whose provider boots a VM through KVM.
const KVM_RUNTIME: &str = "cloudhypervisor";

/// The KVM device.
const KVM: &str = "/dev/kvm";

/// Sticky bit: only a file's owner may remove it, so group write on the directory is harmless.
const STICKY: u32 = 0o1000;

/// Every host check that applies to this config.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    let mut out = vec![egress(i)];
    if i.config.nodes.values().any(|n| n.runtime == KVM_RUNTIME) {
        out.push(kvm_at(Path::new(KVM)));
    }
    out.push(crabbox_config(i));
    out
}

/// The forwarder binary the controller delivers to each Box, when Boxes tunnel to this host.
pub(super) fn egress(i: &Inputs<'_>) -> Check {
    if !keepers_tunnel(i.env) {
        return Check::ok(
            "egress",
            "Boxes reach a remote egress Proxy; no forwarder needed",
        );
    }
    let bin = route::binary(i.env);
    let runnable = fs::metadata(&bin).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0);
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

/// Whether the current user can open the KVM device at `dev` for reading and writing.
pub(super) fn kvm_at(dev: &Path) -> Check {
    match OpenOptions::new().read(true).write(true).open(dev) {
        Ok(_) => Check::ok("kvm", format!("{} is readable and writable", dev.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Check::fail(
            "kvm",
            format!("{} not found", dev.display()),
            "use a host with hardware virtualization and load the kvm module",
        ),
        Err(e) => Check::fail(
            "kvm",
            format!("cannot open {} for read and write: {e}", dev.display()),
            "add this user to the kvm group, then log in again",
        ),
    }
}

/// Where crabbox reads its user config.
fn crabbox_config_path(i: &Inputs<'_>) -> Option<PathBuf> {
    if let Some(p) = i.env.get("CRABBOX_CONFIG").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let fallback = if cfg!(target_os = "macos") {
        "Library/Application Support"
    } else {
        ".config"
    };
    i.env
        .xdg("XDG_CONFIG_HOME", fallback, "crabbox/config.yaml")
}

/// The crabbox user config is mode 0600 and no directory above it is group-writable, which
/// crabbox's own `doctor` insists on.
pub(super) fn crabbox_config(i: &Inputs<'_>) -> Check {
    let Some(path) = crabbox_config_path(i) else {
        return Check::ok(
            "crabbox-cfg",
            "no home directory; crabbox uses its defaults",
        );
    };
    let Ok(meta) = fs::metadata(&path) else {
        return Check::ok(
            "crabbox-cfg",
            format!("no file at {}; crabbox uses its defaults", path.display()),
        );
    };
    let bits = meta.mode() & 0o777;
    if bits != 0o600 {
        return Check::fail(
            "crabbox-cfg",
            format!("{} has mode {bits:o}; it must be 0600", path.display()),
            format!("chmod 600 {}", path.display()),
        );
    }
    let open = path
        .ancestors()
        .skip(1)
        .filter(|d| !d.as_os_str().is_empty())
        .find(|d| fs::metadata(d).is_ok_and(|m| m.mode() & 0o020 != 0 && m.mode() & STICKY == 0));
    if let Some(dir) = open {
        return Check::fail(
            "crabbox-cfg",
            format!("{} is group-writable", dir.display()),
            format!("chmod go-w {}", dir.display()),
        );
    }
    Check::ok("crabbox-cfg", format!("{} is mode 0600", path.display()))
}
