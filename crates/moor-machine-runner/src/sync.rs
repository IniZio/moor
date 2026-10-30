//! `sync`: Box work tree to a host mirror (through the controller), then the mirror to the
//! machine: rsync over ssh, or for a linked machine over its agent's link.

use std::path::Path;
use std::process::Command;

use moor_client::Client;
use moor_core::config::check_excludes;
use moor_core::request::{BoxMirrorReq, Request};
use serde::{Deserialize, Serialize};

use moor_client::link::ops;

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
    /// Files the host mirror took from the Box; unchanged files are not counted.
    pub(crate) files_from_box: u64,
}

/// Mirrors the caller's Box into `mirrors/<box id>`, then that mirror onto the machine, except
/// the caller's `sync_exclude` and the call's `exclude`, which both hops honor.
pub(crate) fn run(
    client: &Client,
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
    let request = Request::BoxMirror(BoxMirrorReq {
        id: caller.box_id,
        dest: mirror.display().to_string(),
        exclude: exclude.clone(),
    });
    let response = client
        .request(request, |_| {})
        .map_err(|e| format!("the controller did not mirror the Box: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the controller refused to mirror the Box: {}",
            error.message
        ));
    }
    let files_from_box = response
        .value
        .as_ref()
        .and_then(|v| v.get("files"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let remote = caller.remote_dir();
    let mut rsync = Command::new("rsync");
    rsync
        .args(["-a", "--delete", "--exclude=.git"])
        .args(exclude.iter().map(|p| format!("--exclude={p}")));
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

/// Points `rsync` at the caller's machine: its ssh target, or the agent link through the
/// runner's rsh helper, which serves the sync while the returned [`Syncing`] is held.
fn reach(
    rsync: &mut Command,
    rsh: Option<&Rsh>,
    caller: &Caller,
    remote: &str,
) -> Result<(String, Option<Syncing>), String> {
    match &caller.reach {
        Reach::Ssh { ssh } => {
            rsync.args(["-e", "ssh -o BatchMode=yes"]).arg(format!(
                "--rsync-path={}",
                caller
                    .route
                    .rsync_path(&format!("mkdir -p {remote} && rsync"))
            ));
            Ok((format!("{ssh}:{remote}/"), None))
        }
        Reach::Linked {
            machine_id,
            machine,
        } => {
            let rsh = rsh.ok_or_else(|| {
                format!("machine {machine} is offline: this runner takes no agent links")
            })?;
            let exe = std::env::current_exe()
                .map_err(|e| format!("cannot find the runner's own binary: {e}"))?;
            let helper = ops::quote(&exe.display().to_string());
            rsync
                .arg("-e")
                .arg(format!("{helper} rsh {}", caller.box_id.get()))
                .env(SOCKET_ENV, &rsh.socket);
            let syncing = rsh.syncs.start(caller.box_id.get(), *machine_id, machine);
            Ok((format!("link:{remote}/"), Some(syncing)))
        }
    }
}
