//! Steps moor runs on the Mac host, outside the sandbox that setup, run and the agent share.
//!
//! The sandbox has no Docker socket and never gets one, so a repo's backend containers start
//! here: a plain argv through the Driver, with no sandbox launcher in front of it.

use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::task_key::TaskKey;
use moor_core::{BoxHandle, BoxPath, ErrorKind, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::lease::LeaseKind;

mod parse;

use self::parse::{classify, holder, Clash};

/// Wall-clock allowance for `docker compose up -d`, which may pull images.
const UP_DEADLINE: Duration = Duration::from_mins(15);

/// Wall-clock allowance for finding which container holds a port.
const PS_DEADLINE: Duration = Duration::from_secs(30);

/// Where the host steps of one Task run.
pub(crate) struct HostSite<'a> {
    /// The Box the steps are tied to.
    pub handle: &'a BoxHandle,
    /// The Box workdir, where `docker compose` finds the repo's compose file.
    pub workdir: &'a BoxPath,
    /// The Task's key; names the compose project.
    pub key: &'a TaskKey,
    /// Host worktree the Box belongs to.
    pub worktree: &'a Path,
}

/// Starts the repo's backend as compose project `moor-<key>` on the host, in the Box workdir.
///
/// The lease row is planned before the exec, so a crash mid-start still gets `down` at release,
/// and held once `up` succeeds. A failed start leaves the row planned.
///
/// # Errors
/// A `Failed` error naming the port or container clash, or the last stderr line.
pub(crate) fn backend_up(
    cx: &Cx,
    id: TaskId,
    site: &HostSite<'_>,
    cfg: &TaskSettings,
) -> Result<(), CommandError> {
    let project = format!("moor-{}", site.key.as_str());
    let store = cx.store()?;
    let lease = store.plan_lease(id, LeaseKind::Compose, &project)?;
    let step = Step {
        cx,
        site,
        class: cfg.size.value.as_str(),
        log_dir: cx.state_dir()?.join("host-steps"),
        id,
    };
    let up = ["docker", "compose", "-p", &project, "up", "-d"];
    let (outcome, log) = step.run("up", &up, UP_DEADLINE);
    let reason = match outcome {
        Ok(ExecOutcome::Exited { code: 0 }) => {
            store.hold_lease(lease)?;
            return Ok(());
        }
        Ok(ExecOutcome::Exited { .. }) => None,
        Ok(ExecOutcome::Interrupted) => Some("interrupted".to_owned()),
        Err(e) => Some(e.to_string()),
    };
    let message = match (reason, classify(&log)) {
        (Some(r), _) => failed(&project, &r),
        (None, Clash::Port(port)) => port_message(&step, &port),
        (None, Clash::Name { name, holder }) => name_message(&name, holder.as_deref()),
        (None, Clash::Other) => failed(&project, log.lines().last().unwrap_or("no output")),
    };
    Err(CommandError::new(ErrorKind::Failed, message))
}

/// The message for a start that failed for a reason other than a clash.
fn failed(project: &str, why: &str) -> String {
    format!("host step backend: docker compose -p {project} up -d failed: {why}")
}

/// The message for a container name that is taken.
fn name_message(name: &str, holder: Option<&str>) -> String {
    let by = holder.map_or_else(String::new, |h| format!(" by container \"{h}\""));
    format!(
        "host step backend: container name \"{name}\" is already in use{by}; \
         remove that container or rename the service"
    )
}

/// Names the port and, when `docker ps` finds it, the container and compose project holding it.
fn port_message(step: &Step<'_>, port: &str) -> String {
    let filter = format!("publish={port}");
    let format = "{{.Names}}\\t{{.Label \"com.docker.compose.project\"}}";
    let ps = ["docker", "ps", "--filter", &filter, "--format", format];
    let (outcome, out) = step.run("ps", &ps, PS_DEADLINE);
    let who = match outcome {
        Ok(ExecOutcome::Exited { code: 0 }) => holder(&out),
        _ => None,
    };
    let held = match who {
        Some((name, Some(project))) => {
            format!("container \"{name}\" (compose project \"{project}\")")
        }
        Some((name, None)) => format!("container \"{name}\""),
        None => "another process or container".to_owned(),
    };
    format!("host step backend: port {port} is held by {held}; stop it or change the port")
}

/// Where and how each host exec runs.
struct Step<'a> {
    /// Context holding the Driver.
    cx: &'a Cx,
    /// Where the exec runs.
    site: &'a HostSite<'a>,
    /// Size class the Box was acquired with.
    class: &'a str,
    /// Directory the exec logs go in.
    log_dir: PathBuf,
    /// Owning Task.
    id: TaskId,
}

impl Step<'_> {
    /// Runs `argv` unsandboxed in the workdir; returns how it ended and its combined output.
    fn run(
        &self,
        name: &str,
        argv: &[&str],
        deadline: Duration,
    ) -> (Result<ExecOutcome, DriverError>, String) {
        let log = self.log_dir.join(format!("{}-{name}.log", self.id.get()));
        let spec = ExecSpec {
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            env: Vec::new(),
            cwd: Some(self.site.workdir.clone()),
            sync: SyncMode::None,
            output: Some(Output::Log(log.clone())),
            host_cwd: Some(self.site.worktree.to_path_buf()),
            class: Some(self.class.to_owned()),
        };
        let prepared = DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.log_dir)
            .and_then(|()| {
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&log)
                    .map(drop)
            });
        if let Err(e) = prepared {
            let err = DriverError::Failed {
                summary: format!("could not create the host step log: {e}"),
            };
            return (Err(err), String::new());
        }
        let outcome = self
            .cx
            .driver()
            .exec(self.site.handle, &spec, Deadline(deadline));
        (outcome, std::fs::read_to_string(&log).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests;
