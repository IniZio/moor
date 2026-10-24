//! Learns a Box's workdir from what crabbox reports when it leases the Box.

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{Acquired, DriverError};
use moor_core::seam::proc::ProcSpec;

use super::errors::Op;
use super::{parse, split_handle, unreadable, Crabbox};

/// Runs the warmup `spec` for `handle` and reads where its workdir is: the ready line's
/// `workdir`, else crabbox's own `<workroot>/<lease id>/<repo>`, where the repo is the lease dir
/// moor runs crabbox from, named after the Box. With neither, the acquire fails: moor never
/// searches the Box for a directory by name.
pub(super) fn warm_up(
    cb: &Crabbox,
    provider: &str,
    handle: &BoxHandle,
    spec: &ProcSpec,
) -> Result<Acquired, DriverError> {
    let out = cb.call(Op::Acquire, Some(provider), spec)?;
    let warm = parse::parse_warmup(&out.stdout).ok_or_else(|| unreadable("acquire"))?;
    if let Some(root) = warm.workroot.as_deref().filter(|r| !r.starts_with('/')) {
        return Err(DriverError::Failed {
            summary: format!(
                "crabbox reported the relative work root {root:?}; it must resolve it against the login home"
            ),
        });
    }
    let derived = warm.workroot.as_deref().map(|root| {
        format!(
            "{}/{}/{}",
            root.trim_end_matches('/'),
            warm.id,
            split_handle(handle).1
        )
    });
    let workdir = [warm.workdir, derived]
        .into_iter()
        .flatten()
        .find_map(|dir| BoxPath::restore(&dir).ok())
        .ok_or_else(|| DriverError::Failed {
            summary: "crabbox reported neither a workdir nor a workroot for the Box".to_owned(),
        })?;
    Ok(Acquired {
        handle: handle.clone(),
        workdir,
    })
}
