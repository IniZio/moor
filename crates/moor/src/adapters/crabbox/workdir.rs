//! Learns a Box's workdir by asking the Box.

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Acquired, DriverError};
use moor_core::seam::proc::ProcSpec;

use super::errors::{map_error, Op};
use super::{parse, split_handle, target_args, unreadable, Crabbox, QUICK_DEADLINE};

/// Prints the directory crabbox starts commands in.
const PROBE: &str = r#"printf "MOOR_WORKDIR=%s\n" "$(pwd)""#;
/// Prefix of the line that carries the directory.
const MARKER: &str = "MOOR_WORKDIR=";

/// Probes the workdir, because crabbox derives it from its lease id and the repo name.
pub(super) fn probe(cb: &Crabbox, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
    let spec = Crabbox::spec(QUICK_DEADLINE)
        .arg("run")
        .args(target_args(handle))
        .arg("--no-hydrate")
        .arg("--no-sync");
    let spec = cb
        .at_lease(spec, handle)?
        .arg("--")
        .args(["sh", "-c", PROBE]);
    let out = cb
        .retry_claim_race(|| cb.proc.run(&spec))
        .map_err(|e| map_error(Op::Workdir, split_handle(handle).0, &e))?;
    let path = out
        .stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(MARKER))
        .map(|p| p.trim_end_matches('\r'))
        .ok_or_else(|| DriverError::Failed {
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

/// Runs the warmup `spec` for `handle` and reads where its workdir is.
pub(super) fn warm_up(
    cb: &Crabbox,
    provider: &str,
    handle: &BoxHandle,
    spec: &ProcSpec,
) -> Result<Acquired, DriverError> {
    let out = cb.call(Op::Acquire, Some(provider), spec)?;
    let warm = parse::parse_warmup(&out.stdout).ok_or_else(|| unreadable("acquire"))?;
    let workdir = match warm.workdir.as_deref().map(BoxPath::restore) {
        Some(Ok(dir)) => dir,
        _ => probe(cb, handle)?,
    };
    Ok(Acquired {
        handle: handle.clone(),
        workdir,
    })
}
