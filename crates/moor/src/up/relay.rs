//! The egress relay of a rootless Docker daemon, which `up` runs as a user unit inside the
//! daemon's network namespace.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use moor_client::rootless;

use super::files::Layout;
use super::node::{Plan, INSTALL};
use super::service;
use crate::doctor::Check;
use crate::env::Env;

/// The rootless Docker daemon's own user unit.
const DOCKER_UNIT: &str = "docker.service";
/// What the egress relay of a rootless daemon needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relay {
    /// The `moor-egress-forward` executable, which has a relay mode.
    pub forward: PathBuf,
    /// The state dir of the daemon's rootlesskit, which holds `child_pid`.
    pub state: PathBuf,
    /// The user unit that runs the daemon.
    pub docker: String,
}

/// The rootless daemon's namespace address, which the relay listens on and a Box dials, and the
/// relay's own facts.
pub(super) fn relayed(env: &Env) -> Result<(Ipv4Addr, Relay), Check> {
    let fail = |why: String| Check::fail("relay", why, "check that the rootless daemon is running");
    let state = rootless::daemon_state().map_err(fail)?;
    let ip = rootless::child_ip(&state).map_err(fail)?;
    let forward = crate::proxy::route::binary(env);
    if !fs::metadata(&forward).is_ok_and(|m| m.is_file()) {
        return Err(Check::fail(
            "relay",
            format!("{} does not exist", forward.display()),
            INSTALL,
        ));
    }
    let docker = DOCKER_UNIT.to_owned();
    Ok((
        ip,
        Relay {
            forward,
            state,
            docker,
        },
    ))
}

/// The relay's unit text: it enters the rootless daemon's namespace as it is when the unit starts,
/// and follows the daemon's own unit. `PartOf` restarts it with the daemon and `BindsTo` stops it
/// when the daemon dies; `WantedBy` the daemon's unit starts it again when systemd restarts a
/// crashed daemon, which a stop by dependency does not.
pub(crate) fn relay_unit(
    plan: &Plan,
    layout: &Layout,
    relay: &Relay,
    path: Option<&str>,
) -> String {
    let script = "exec nsenter -U --preserve-credentials -n -t $(cat \"$1\"/child_pid) \"$2\" --relay \"$3\" --socket \"$4\"";
    let words = [
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
        "moor-relay".to_owned(),
        relay.state.display().to_string(),
        relay.forward.display().to_string(),
        plan.egress.to_string(),
        crate::proxy::route::local_socket(&layout.state_dir)
            .display()
            .to_string(),
    ];
    let exec = words
        .iter()
        .map(|w| service::quoted(&w.replace('$', "$$")))
        .collect::<Vec<_>>()
        .join(" ");
    let vars = path.map_or_else(String::new, |p| {
        format!("Environment={}\n", service::quoted(&format!("PATH={p}")))
    });
    let docker = &relay.docker;
    format!(
        "[Unit]
Description=moor egress relay: carries Box egress into the rootless Docker namespace
BindsTo={docker}
PartOf={docker}
After={docker}

[Service]
ExecStart={exec}
{vars}Restart=always
RestartSec=2

[Install]
WantedBy=default.target {docker}
"
    )
}

/// The plan's `found` checks, plus one when this daemon needs no relay and the relay unit an earlier
/// rootless `up` left in `dir` was stopped and removed. It runs before the controller unit is installed.
pub(super) fn settle(
    mut found: Vec<Check>,
    relay: Option<&Relay>,
    dir: &Path,
) -> Result<Vec<Check>, Check> {
    if relay.is_none() && service::remove_units(dir, &[service::RELAY])? {
        found.push(Check::ok(
            "relay",
            format!("removed {}, which a rootless up left", service::RELAY),
        ));
    }
    Ok(found)
}
