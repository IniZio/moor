//! Learns a Box's workdir by asking the Box.

use std::path::Path;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;

use super::errors::{map_error, Op};
use super::{split_handle, static_env, target_args, Crabbox, QUICK_DEADLINE};

/// Prints the directory crabbox starts commands in.
const PROBE: &str = r#"printf "MOOR_WORKDIR=%s\n" "$(pwd)""#;
/// Prefix of the line that carries the directory.
const MARKER: &str = "MOOR_WORKDIR=";

/// Probes the workdir, because crabbox derives it from its lease id and the repo name.
pub(super) fn probe(
    cb: &Crabbox,
    handle: &BoxHandle,
    host_cwd: Option<&Path>,
) -> Result<BoxPath, DriverError> {
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
