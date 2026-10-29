//! `pull`: one file from the machine to the Box's work tree. rsync over ssh brings it to a
//! staging file on the host, then the controller's `box-put` writes it into the Box.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use moor_client::Client;
use moor_core::name::BoxPath;
use moor_core::request::{BoxPutReq, Request};
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

use crate::beat::Progress;
use crate::caller::Caller;

static CALLS: AtomicU64 = AtomicU64::new(0);

/// Arguments of `pull`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct PullArgs {
    /// A file on the machine: absolute, or relative to the Box's copy at `~/moor/<box id>`.
    pub(crate) path: String,
    /// Where the file lands, relative to the Box's work tree. An existing file is replaced.
    pub(crate) dest: String,
}

impl PullArgs {
    /// Refuses a `dest` outside the work tree and a `path` that cannot name a file.
    pub(crate) fn check(&self) -> Result<(), String> {
        BoxPath::under("/", &self.dest).map_err(|_| {
            format!(
                "dest {:?} must be a path inside the work tree: not empty, not absolute, no `..`",
                self.dest
            )
        })?;
        if self.path.is_empty() || self.path.contains(['\n', '\0']) {
            return Err(format!("path {:?} does not name a file", self.path));
        }
        Ok(())
    }
}

/// What a pull did.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct PullOutput {
    /// Where the file landed, relative to the work tree.
    pub(crate) dest: String,
    /// Bytes written.
    pub(crate) bytes: u64,
}

/// Removes the staging file when the pull ends, whichever way.
struct Staged(PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs [`run`] off the async threads, reporting progress while it does.
pub(crate) async fn serve(
    client: Client,
    stage: PathBuf,
    caller: Caller,
    args: PullArgs,
    progress: Progress,
    beat: Duration,
) -> Result<PullOutput, ErrorData> {
    args.check()
        .map_err(|e| ErrorData::invalid_params(e, None))?;
    let copy = tokio::task::spawn_blocking(move || run(&client, &stage, &caller, &args));
    tokio::select! {
        done = copy => done
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .map_err(|e| ErrorData::internal_error(e, None)),
        () = progress.beat("pull", beat) => unreachable!("a beat never ends"),
    }
}

/// Copies the file to a staging file under `stage`, then has the controller write it into the
/// caller's Box.
pub(crate) fn run(
    client: &Client,
    stage: &Path,
    caller: &Caller,
    args: &PullArgs,
) -> Result<PullOutput, String> {
    std::fs::create_dir_all(stage)
        .map_err(|e| format!("could not create {}: {e}", stage.display()))?;
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    let staged = Staged(stage.join(format!("{}-{n}", std::process::id())));
    let remote = if args.path.starts_with('/') {
        args.path.clone()
    } else {
        format!("{}/{}", caller.remote_dir(), args.path)
    };
    let ssh = caller.reach.ssh("pulling files")?;
    let out = Command::new("rsync")
        .args(["-L", "-e", "ssh -o BatchMode=yes"])
        .arg(format!("{ssh}:{remote}"))
        .arg(&staged.0)
        .output()
        .map_err(|e| format!("could not run rsync: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rsync from {} failed ({}): {}",
            caller.machine,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !staged.0.is_file() {
        return Err(format!(
            "{} on {} is not a regular file",
            args.path, caller.machine
        ));
    }
    let request = Request::BoxPut(BoxPutReq {
        id: caller.box_id,
        src: staged.0.display().to_string(),
        dest: args.dest.clone(),
    });
    let response = client
        .request(request, |_| {})
        .map_err(|e| format!("the controller did not take the file: {e}"))?;
    if let Some(error) = response.error {
        return Err(format!(
            "the controller refused to write the file: {}",
            error.message
        ));
    }
    let bytes = response
        .value
        .as_ref()
        .and_then(|v| v.get("bytes"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Ok(PullOutput {
        dest: args.dest.clone(),
        bytes,
    })
}
