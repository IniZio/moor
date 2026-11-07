//! Moving the agent to the server's release: at once when the server refuses this one, or in
//! the background while linked when the server runs another, restarting in place only once no
//! stream or boot is going on.

use std::convert::Infallible;
use std::os::unix::process::CommandExt;
use std::process::Command;

use tokio::sync::watch;

use crate::{busy, release, VERSION};

/// Where the agent logs.
pub(crate) type Log<'a> = &'a (dyn Fn(&str) + Sync);

/// The release a restarted agent was just updated to, so that a server still naming it is
/// reported instead of updated for again.
const UPDATED_TO: &str = "MOOR_AGENT_UPDATED_TO";

/// Why `run_with` stops for a server that refuses this release and needs `version`: the update
/// failed, the restart failed, or updating was not asked for.
pub(crate) async fn required(version: &str, auto_update: bool, log: Log<'_>) -> String {
    let need = format!("the server needs this agent updated from {VERSION} to {version}");
    if !auto_update {
        return release::plain_semver(version).map_or_else(
            |_| format!("{need}, which is not a release this agent can install"),
            |plain| format!(
                "{need}; run `moor-machine-agent update --version {plain}`, or start `run` with --auto-update"
            ),
        );
    }
    install(need, version, log).await
}

/// Installs release `version`, then restarts into it with the same arguments once no work is
/// going on; why it did not.
async fn install(need: String, version: &str, log: Log<'_>) -> String {
    let Ok(plain) = release::plain_semver(version) else {
        return format!("{need}, which is not a release this agent can install");
    };
    if std::env::var(UPDATED_TO).is_ok_and(|done| done == plain) {
        return format!("{need}, but it was just updated to {plain}");
    }
    log(&format!("{need}; updating"));
    let asked = plain.clone();
    let done = match tokio::task::spawn_blocking(move || release::update(&asked)).await {
        Ok(Ok(done)) => done,
        Ok(Err(e)) => return format!("{need}; the update failed: {e}"),
        Err(e) => return format!("{need}; the update failed: {e}"),
    };
    log(&format!(
        "updated from {} to {}; restarting once no stream or boot is going on",
        done.from, done.to
    ));
    let why = busy::when_idle(|| {
        Command::new(&done.exe)
            .args(std::env::args_os().skip(1))
            .env(UPDATED_TO, &done.to)
            .exec()
    })
    .await;
    format!(
        "updated to {plain} but cannot restart {}: {why}",
        done.exe.display()
    )
}

/// Follows the release of each server that links this agent, which `offers` names as each link
/// comes up: installs it and restarts into it with `auto_update`, or only says it exists. A
/// failed update leaves this release running. Never ends.
pub(crate) async fn follow(
    mut offers: watch::Receiver<String>,
    auto_update: bool,
    log: Log<'_>,
) -> Infallible {
    loop {
        if offers.changed().await.is_err() {
            return std::future::pending().await;
        }
        let theirs = offers.borrow_and_update().clone();
        if theirs == VERSION {
            continue;
        }
        let need = format!("the server runs {theirs} and this agent {VERSION}");
        if !auto_update {
            log(&format!(
                "{need}; run `moor-machine-agent update --version {theirs}`, or start `run` with --auto-update"
            ));
            continue;
        }
        let why = install(need, &theirs, log).await;
        log(&format!("{why}; this agent keeps running {VERSION}"));
    }
}
