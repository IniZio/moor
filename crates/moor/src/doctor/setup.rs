//! The config and controller checks.

use super::{answer, Check, Inputs};
use crate::client::Target;
use crate::config_load::user_path;

/// Where `target` is, for a message.
pub(super) fn place(target: &Target) -> String {
    match target {
        Target::Socket(p) => p.display().to_string(),
        Target::Ssh(host) => format!("ssh {host}"),
    }
}

/// Which config file was read and whether a controller is set.
pub(super) fn config(i: &Inputs<'_>) -> Check {
    let path = user_path(i.env);
    let shown = path.as_ref().map_or_else(
        || "no config location (HOME is unset)".to_owned(),
        |p| p.display().to_string(),
    );
    if let Some(why) = i.config_error {
        return Check::fail("config", why, format!("fix the file at {shown}"));
    }
    let file = match &path {
        Some(p) if p.is_file() => format!("read {shown}"),
        _ => format!("no config file at {shown}, using defaults"),
    };
    i.target.map_or_else(
        || {
            Check::warn(
                "config",
                format!("{file}; no controller set, so commands run in-process"),
                "set controller.socket to use a running controller",
            )
        },
        |t| Check::ok("config", format!("{file}; controller {}", place(t))),
    )
}

/// Whether the controller answers and runs this CLI's version.
pub(super) fn controller(i: &Inputs<'_>) -> Check {
    let Some(target) = i.target else {
        return Check::warn(
            "controller",
            "none set: commands run in-process",
            "set controller.socket to use a running controller",
        );
    };
    let version = answer::<serde_json::Value>(i.ask, moor_core::Request::Version {});
    let theirs = match version {
        Ok(v) => v
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        Err(p) => return Check::failed("controller", p),
    };
    let ours = env!("CARGO_PKG_VERSION");
    let at = place(target);
    if theirs == ours {
        Check::ok("controller", format!("moor {theirs} at {at}"))
    } else {
        Check::warn(
            "controller",
            format!("moor {theirs} at {at}, but this CLI is {ours}"),
            "install the same moor release on both",
        )
    }
}
