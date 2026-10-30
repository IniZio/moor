//! `pull`: one file from the machine to the Box's work tree. rsync over ssh, or the agent's
//! link for a linked machine, brings it to a staging file on the host, then the controller's
//! `box-put` writes it into the Box.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use moor_client::link::ops::{Op, MAX_PULL};
use moor_client::Client;
use moor_core::name::BoxPath;
use moor_core::request::{BoxPutReq, Request};
use moor_core::BoxId;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use crate::caller::{Caller, Reach};
use crate::link::{self, Answer};
use crate::route::Route;
use crate::Agents;

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

/// Copies the file to a staging file under `stage`, then has the controller write it into the
/// caller's Box.
pub(crate) async fn serve(
    client: Client,
    agents: Option<Agents>,
    stage: PathBuf,
    caller: Caller,
    args: PullArgs,
) -> Result<PullOutput, ErrorData> {
    args.check()
        .map_err(|e| ErrorData::invalid_params(e, None))?;
    let pull = async {
        std::fs::create_dir_all(&stage)
            .map_err(|e| format!("could not create {}: {e}", stage.display()))?;
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        let staged = Staged(stage.join(format!("{}-{n}", std::process::id())));
        match &caller.reach {
            Reach::Ssh { ssh } => {
                let (ssh, from, path, to) = (
                    ssh.clone(),
                    caller.clone(),
                    args.path.clone(),
                    staged.0.clone(),
                );
                tokio::task::spawn_blocking(move || over_ssh(&ssh, &from, &path, &to))
                    .await
                    .map_err(|e| e.to_string())??;
            }
            Reach::Linked {
                machine_id,
                machine,
            } => {
                let op = Op::Pull {
                    box_id: caller.box_id.get(),
                    path: args.path.clone(),
                };
                over_link(agents.as_ref(), machine_id, machine, &op, &staged.0).await?;
            }
        }
        if !staged.0.is_file() {
            return Err(format!(
                "{} on {} is not a regular file",
                args.path, caller.machine
            ));
        }
        let (box_id, dest) = (caller.box_id, args.dest.clone());
        tokio::task::spawn_blocking(move || put(&client, &staged, box_id, dest))
            .await
            .map_err(|e| e.to_string())?
    };
    pull.await.map_err(|e| ErrorData::internal_error(e, None))
}

/// Copies `path`, absolute or relative to the Box's copy, from `ssh` to `to`.
fn over_ssh(ssh: &str, caller: &Caller, path: &str, to: &Path) -> Result<(), String> {
    let remote = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{}", caller.remote_dir(), path)
    };
    let out = Command::new("rsync")
        .args(["-L", "-e", "ssh -o BatchMode=yes"])
        .args(match &caller.route {
            Route::Host => Vec::new(),
            // -s keeps the path out of the remote command, which two shells would split.
            route @ Route::Vm(_) => vec![
                "-s".to_owned(),
                format!("--rsync-path={}", route.rsync_path("rsync")),
            ],
        })
        .arg(format!("{ssh}:{remote}"))
        .arg(to)
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
    Ok(())
}

/// Sends the pull `op` to machine `machine_id` and writes the file it sends to `to`.
async fn over_link(
    agents: Option<&Agents>,
    machine_id: &i64,
    machine: &str,
    op: &Op,
    to: &Path,
) -> Result<(), String> {
    let Answer::Sending(stream, bytes) = link::send(agents, machine, *machine_id, op).await? else {
        return Err(format!("machine {machine} sent no file"));
    };
    if bytes > MAX_PULL {
        return Err(format!("machine {machine} sent more than {MAX_PULL} bytes"));
    }
    let mut file = tokio::fs::File::create(to)
        .await
        .map_err(|e| format!("could not create {}: {e}", to.display()))?;
    let got = tokio::io::copy(&mut stream.take(bytes), &mut file)
        .await
        .map_err(|e| format!("machine {machine} broke off the file: {e}"))?;
    if got != bytes {
        return Err(format!(
            "machine {machine} sent {got} of the file's {bytes} bytes"
        ));
    }
    Ok(())
}

/// Has the controller write `staged` into Box `box_id` at `dest`.
fn put(
    client: &Client,
    staged: &Staged,
    box_id: BoxId,
    dest: String,
) -> Result<PullOutput, String> {
    let request = Request::BoxPut(BoxPutReq {
        id: box_id,
        src: staged.0.display().to_string(),
        dest: dest.clone(),
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
    Ok(PullOutput { dest, bytes })
}
