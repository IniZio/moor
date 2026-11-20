//! Moving the agent to the server's release: at once when the server refuses this one, or in
//! the background while linked when the server runs another, restarting in place only once no
//! stream or boot is going on.

use std::convert::Infallible;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::sync::watch;

use crate::{busy, release, VERSION};

/// Where the agent logs.
pub(crate) type Log<'a> = &'a (dyn Fn(&str) + Sync);

/// The release a restarted agent was just updated to, so that a server still naming it is
/// reported instead of updated for again.
const UPDATED_TO: &str = "MOOR_AGENT_UPDATED_TO";
/// How long a restart waits for work to end before the agent says what holds it.
const PATIENCE: Duration = Duration::from_mins(30);

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
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return format!("{need}; the update failed: cannot find this executable: {e}"),
    };
    if let Err(e) = swap(&exe, &plain).await {
        return format!("{need}; the update failed: {e}");
    }
    log(&format!(
        "updated from {VERSION} to {plain}; restarting once no stream or boot is going on"
    ));
    let to = plain.clone();
    let stuck = |held: &[String]| {
        log(&format!(
            "the update to {to} has waited {} minutes, held by: {}",
            PATIENCE.as_secs() / 60,
            held.join(", ")
        ));
    };
    let why = busy::when_idle(PATIENCE, stuck, || {
        Command::new(&exe)
            .args(std::env::args_os().skip(1))
            .env(UPDATED_TO, &plain)
            .exec()
    })
    .await;
    format!(
        "updated to {plain} but cannot restart {}: {why}",
        exe.display()
    )
}

/// Installs release `version` over `exe` from a child `update`, so no fork of this process
/// holds the new file open for writing when the restart execs it.
async fn swap(exe: &Path, version: &str) -> Result<(), String> {
    let out = tokio::process::Command::new(exe)
        .args(["update", "--version", version])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("cannot run {} update: {e}", exe.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let last = said.lines().last().unwrap_or_default();
    Err(last.trim_start_matches("moor-machine-agent: ").to_owned())
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
