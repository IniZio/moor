//! The host checks: what a Box start needs from this machine beyond crabbox, which checks its own config.

use std::fs;
use std::os::unix::fs::MetadataExt;

use super::{Check, Inputs};
use crate::proxy::route;
use crate::wire::keepers_tunnel;

/// Every host check that applies to this config.
pub(super) fn checks(i: &Inputs<'_>) -> Vec<Check> {
    vec![egress(i)]
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
