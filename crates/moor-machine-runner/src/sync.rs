//! `sync`: Box work tree to a host mirror (through the controller), then the mirror to the
//! machine (rsync over ssh).

use std::path::Path;
use std::process::Command;

use moor_client::Client;
use moor_core::request::{BoxMirrorReq, Request};
use serde::{Deserialize, Serialize};

use crate::caller::Caller;

/// Most `exclude` patterns one sync takes.
const MAX_EXCLUDES: usize = 64;

/// Arguments of `sync`.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct SyncArgs {
    /// rsync patterns, relative to the work tree root, to leave alone on the machine: they are
    /// not copied there and not deleted there. For things the machine builds for itself, such
    /// as `app/node_modules`. At most 64.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
}

impl SyncArgs {
    /// Refuses a pattern rsync would read as an option or that cannot be one argument.
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.exclude.len() > MAX_EXCLUDES {
            return Err(format!(
                "exclude takes at most {MAX_EXCLUDES} patterns, not {}",
                self.exclude.len()
            ));
        }
        for pattern in &self.exclude {
            if pattern.is_empty() || pattern.starts_with('-') || pattern.contains(['\n', '\0']) {
                return Err(format!(
                    "exclude pattern {pattern:?} is refused: it must not be empty, start with \
                     '-' or hold a newline or NUL"
                ));
            }
        }
        Ok(())
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
/// the `exclude` patterns, which only the second hop honors.
pub(crate) fn run(
    client: &Client,
    mirrors: &Path,
    caller: &Caller,
    args: &SyncArgs,
) -> Result<SyncOutput, String> {
    let mirror = mirrors.join(caller.box_id.get().to_string());
    let request = Request::BoxMirror(BoxMirrorReq {
        id: caller.box_id,
        dest: mirror.display().to_string(),
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
    let out = Command::new("rsync")
        .args(["-a", "--delete", "--exclude=.git"])
        .args(args.exclude.iter().map(|p| format!("--exclude={p}")))
        .args(["-e", "ssh -o BatchMode=yes"])
        .arg(format!("--rsync-path=mkdir -p {remote} && rsync"))
        .arg(format!("{}/", mirror.display()))
        .arg(format!("{}:{remote}/", caller.ssh))
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
