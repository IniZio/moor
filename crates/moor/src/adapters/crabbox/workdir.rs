//! Learns a Box's workdir by asking the Box.

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Acquired, BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::proc::{Deadline, ProcSpec};

use super::errors::Op;
use super::{batch, parse, split_handle, unreadable, Crabbox, QUICK_DEADLINE};

/// Prefix of the line that carries the directory.
const MARKER: &str = "MOOR_WORKDIR=";

/// Finds the Box workdir over the Box's shared ssh connection.
///
/// crabbox puts it at `<workroot>/<lease id>/<repo>`, and the repo is the lease dir moor runs
/// crabbox from, named after the Box. Neither the workroot nor the lease id is known here, so
/// the probe asks the Box for the one directory with that name, at most five levels deep.
pub(super) fn probe(cb: &Crabbox, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
    let script = find_script("/", split_handle(handle).1);
    let ran = batch::run(
        cb,
        handle,
        &[],
        &[BoxStep::Run(script)],
        Deadline(QUICK_DEADLINE),
    );
    let stdout = match ran.into_iter().next() {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => stdout,
        Some(Err(e)) => return Err(e),
        _ => {
            return Err(DriverError::Failed {
                summary: "box workdir probe found no single workdir for the Box".into(),
            })
        }
    };
    let path = stdout
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

/// The sh script that prints [`MARKER`] and the one directory named `name` at most five levels
/// under `root`, and exits 1 when there is none or more than one.
pub(super) fn find_script(root: &str, name: &str) -> String {
    format!(
        "d=$(find {} -maxdepth 5 \\( -path /proc -o -path /sys -o -path /dev \\) -prune -o \
         -type d -name {} -print 2>/dev/null | head -n 2)\n\
         test -n \"$d\" && test \"$(printf '%s\\n' \"$d\" | wc -l)\" -eq 1 || exit 1\n\
         printf '{MARKER}%s\\n' \"$d\"\n",
        quote(root),
        quote(name)
    )
}

/// Runs the warmup `spec` for `handle` and reads where its workdir is: the ready line's
/// `workdir`, else crabbox's own `<workroot>/<lease id>/<repo>`, else the probe.
pub(super) fn warm_up(
    cb: &Crabbox,
    provider: &str,
    handle: &BoxHandle,
    spec: &ProcSpec,
) -> Result<Acquired, DriverError> {
    let out = cb.call(Op::Acquire, Some(provider), spec)?;
    let warm = parse::parse_warmup(&out.stdout).ok_or_else(|| unreadable("acquire"))?;
    let derived = warm.workroot.as_deref().map(|root| {
        format!(
            "{}/{}/{}",
            root.trim_end_matches('/'),
            warm.id,
            split_handle(handle).1
        )
    });
    let workdir = match warm.workdir.or(derived).as_deref().map(BoxPath::restore) {
        Some(Ok(dir)) => dir,
        _ => probe(cb, handle)?,
    };
    Ok(Acquired {
        handle: handle.clone(),
        workdir,
    })
}
