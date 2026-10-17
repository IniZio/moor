//! Learns a Box's workdir by asking the Box.

use std::path::Path;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{AcquireReq, BoxPlatform, Driver, DriverError};

use super::errors::{map_error, Op};
use super::parse::Warmup;
use super::{split_handle, static_env, target_args, Crabbox, QUICK_DEADLINE};

/// Prints the directory crabbox starts commands in, then the Box's OS and CPU.
const PROBE: &str = r#"printf "MOOR_WORKDIR=%s\n" "$(pwd)"; printf "MOOR_UNAME=%s %s\n" "$(uname -s)" "$(uname -m)""#;
/// Prefix of the line that carries the directory.
const MARKER: &str = "MOOR_WORKDIR=";
/// Prefix of the line that carries `uname -s` and `uname -m`.
const UNAME_MARKER: &str = "MOOR_UNAME=";

/// Probes the workdir, because crabbox derives it from its lease id and the repo name.
pub(super) fn probe(
    cb: &Crabbox,
    handle: &BoxHandle,
    host_cwd: Option<&Path>,
) -> Result<BoxPath, DriverError> {
    workdir_of(&run_probe(cb, handle, host_cwd)?)
}

/// The workdir and platform of a freshly warmed Box.
///
/// Warmup output names the workdir of a Linux Box, so it needs no probe. A static host may
/// be a Mac, and only the Box says so, so it is always probed.
pub(super) fn acquired(
    cb: &Crabbox,
    handle: &BoxHandle,
    warm: &Warmup,
    req: &AcquireReq,
) -> Result<(BoxPath, BoxPlatform), DriverError> {
    let warm_dir = warm
        .workdir
        .as_deref()
        .and_then(|d| BoxPath::restore(d).ok());
    if let Some(dir) = &warm_dir {
        if !cb.caps(&req.provider).is_ok_and(|c| c.static_host) {
            return Ok((dir.clone(), BoxPlatform::linux()));
        }
    }
    let (probed, platform) = probe_platform(cb, handle, req.worktree.as_deref())?;
    Ok((warm_dir.unwrap_or(probed), platform))
}

/// Probes the workdir and the platform in one remote call.
pub(super) fn probe_platform(
    cb: &Crabbox,
    handle: &BoxHandle,
    host_cwd: Option<&Path>,
) -> Result<(BoxPath, BoxPlatform), DriverError> {
    let stdout = run_probe(cb, handle, host_cwd)?;
    let workdir = workdir_of(&stdout)?;
    let platform = last_line(&stdout, UNAME_MARKER)
        .and_then(BoxPlatform::parse)
        .ok_or_else(|| DriverError::Failed {
            summary: "box probe printed no readable MOOR_UNAME line".into(),
        })?;
    Ok((workdir, platform))
}

/// Runs the probe and returns its stdout.
fn run_probe(
    cb: &Crabbox,
    handle: &BoxHandle,
    host_cwd: Option<&Path>,
) -> Result<String, DriverError> {
    let mut spec = Crabbox::spec(QUICK_DEADLINE)
        .arg("run")
        .args(target_args(handle))
        .arg("--no-hydrate")
        .arg("--no-sync");
    if let Some(dir) = host_cwd {
        spec = spec.cwd(dir);
    }
    let spec = static_env(spec, handle).arg("--").args(["sh", "-c", PROBE]);
    let out = cb
        .retry_claim_race(|| cb.proc.run(&spec))
        .map_err(|e| map_error(Op::Workdir, split_handle(handle).0, &e))?;
    Ok(out.stdout)
}

/// The value after the last line starting with `marker`.
fn last_line<'a>(stdout: &'a str, marker: &str) -> Option<&'a str> {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(marker))
        .map(|v| v.trim_end_matches('\r'))
}

/// Reads and validates the directory line.
fn workdir_of(stdout: &str) -> Result<BoxPath, DriverError> {
    let path = last_line(stdout, MARKER).ok_or_else(|| DriverError::Failed {
        summary: "box workdir probe printed no MOOR_WORKDIR line".into(),
    })?;
    if !path.starts_with('/') {
        return Err(DriverError::Failed {
            summary: "box workdir probe reported a relative path".into(),
        });
    }
    BoxPath::restore(path).map_err(|_| DriverError::Failed {
        summary: "box workdir probe reported an unusable path".into(),
    })
}
