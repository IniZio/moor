//! `sync`: Box work tree to a mirror in the runner's state directory, by rsync through the
//! controller, then the mirror to the machine over its agent's link.

use std::path::Path;
use std::process::Command;

use moor_core::config::check_excludes;
use serde::{Deserialize, Serialize};

use moor_client::link::ops;

use crate::box_rsh::{self, Dial};
use crate::caller::{Caller, Reach};
use crate::rsh::{Rsh, Syncing, SOCKET_ENV};

/// Arguments of `sync`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct SyncArgs {
    /// rsync patterns, relative to the work tree root, to leave alone on the machine: they are
    /// not copied there and not deleted there. For things the machine builds for itself, such
    /// as `app/build`. Added to `machine_info`'s `sync_exclude`, which always applies. At most 64.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
}

impl SyncArgs {
    /// Refuses a pattern rsync would read as an option or that cannot be one argument.
    pub(crate) fn check(&self) -> Result<(), String> {
        check_excludes(&self.exclude)
    }
}

/// What a sync did.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct SyncOutput {
    /// The machine synced to.
    pub(crate) machine: String,
    /// The Box's copy on the machine, under the machine user's home.
    pub(crate) path: String,
    /// Files the mirror took from the Box; unchanged files are not counted.
    pub(crate) files_from_box: u64,
}

/// Mirrors the caller's Box into `mirrors/<box id>`, then that mirror onto the machine, except
/// the caller's `sync_exclude` and the call's `exclude`, which both hops honor.
pub(crate) fn run(
    dial: &Dial,
    mirrors: &Path,
    rsh: Option<&Rsh>,
    caller: &Caller,
    args: &SyncArgs,
) -> Result<SyncOutput, String> {
    let mirror = mirrors.join(caller.box_id.get().to_string());
    let exclude: Vec<String> = caller
        .sync_exclude
        .iter()
        .chain(&args.exclude)
        .cloned()
        .collect();
    let files_from_box = mirror_box(dial, caller, &exclude, &mirror)?;
    let remote = caller.remote_dir();
    let mut rsync = rsync(&exclude);
    let (target, _syncing) = reach(&mut rsync, rsh, caller, &remote)?;
    let out = rsync
        .arg(format!("{}/", mirror.display()))
        .arg(target)
        .output()
        .map_err(|e| format!("could not run rsync: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rsync to {} failed ({}): {}",
            caller.machine,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(SyncOutput {
        machine: caller.machine.clone(),
        path: format!("~/{remote}"),
        files_from_box,
    })
}

/// Makes `mirror` an exact copy of the caller's Box's work tree but `.git` and `exclude`; the
/// files it took. A mirror lost with the runner's state comes back whole on the next sync.
fn mirror_box(
    dial: &Dial,
    caller: &Caller,
    exclude: &[String],
    mirror: &Path,
) -> Result<u64, String> {
    std::fs::create_dir_all(mirror)
        .map_err(|e| format!("could not create {}: {e}", mirror.display()))?;
    let out = box_rsh::rsync(dial, caller.box_id)?
        .args(["-a", "--delete", "--stats", "--exclude=.git"])
        .args(exclude.iter().map(|p| format!("--exclude={p}")))
        .arg("--")
        .arg("box:./")
        .arg(format!("{}/", mirror.display()))
        .output()
        .map_err(|e| format!("could not run rsync: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rsync from the Box failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stats = String::from_utf8_lossy(&out.stdout);
    Ok(stats
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("Number of regular files transferred:")
        })
        .and_then(|v| v.trim().replace(',', "").parse().ok())
        .unwrap_or(0))
}

/// Points `rsync` at the caller's machine's agent link through the runner's rsh helper, which serves the sync while the returned [`Syncing`] is held.
fn reach(
    rsync: &mut Command,
    rsh: Option<&Rsh>,
    caller: &Caller,
    remote: &str,
) -> Result<(String, Option<Syncing>), String> {
    let Reach {
        machine_id,
        machine,
    } = &caller.reach;
    let rsh = rsh
        .ok_or_else(|| format!("machine {machine} is offline: this runner takes no agent links"))?;
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot find the runner's own binary: {e}"))?;
    let helper = ops::quote(&exe.display().to_string());
    rsync
        .arg("-e")
        .arg(format!("{helper} rsh {}", caller.box_id.get()))
        .env(SOCKET_ENV, &rsh.socket);
    let syncing = rsh.syncs.start(caller.box_id.get(), *machine_id, machine);
    Ok((format!("link:{remote}/"), Some(syncing)))
}

/// The rsync that copies the mirror to the machine, leaving out `exclude`; it and the remote
/// shell it runs never see the service key.
fn rsync(exclude: &[String]) -> Command {
    let mut rsync = Command::new("rsync");
    rsync
        .env_remove(crate::SERVICE_KEY_ENV)
        .args(["-a", "--delete", "--exclude=.git"])
        .args(exclude.iter().map(|p| format!("--exclude={p}")));
    rsync
}

#[cfg(test)]
mod tests {
    #[test]
    fn rsync_never_inherits_the_service_key() {
        let rsync = super::rsync(&[]);
        let removed: Vec<_> = rsync.get_envs().filter(|(_, v)| v.is_none()).collect();
        assert_eq!(
            removed,
            [(std::ffi::OsStr::new(crate::SERVICE_KEY_ENV), None)]
        );
    }
}
