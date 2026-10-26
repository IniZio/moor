//! `box-mirror`: make a host directory an exact copy of a Box's work tree.
//!
//! Holds the Box lock for the mirror, as `cp` does for its copy.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::DriverError;
use moor_core::view::run::MirrorValue;
use moor_core::{BoxMirrorReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::host_path::absolute;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::wake::wake_and_revive;
use crate::ops::cp::usable_handle;

/// Run `box-mirror`.
pub(crate) fn run(cx: &Cx, req: &BoxMirrorReq) -> Result<MirrorValue, CommandError> {
    let to = absolute("the destination", &req.dest)?;
    let store = cx.store()?;
    let id = req.id;
    let not_open = || {
        CommandError::new(
            ErrorKind::Failed,
            format!("no open Box with id {}", id.get()),
        )
    };
    let open = |row: &crate::store::BoxRow| {
        !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
    };
    store.box_row(id)?.ok_or_else(not_open)?;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "box-mirror", boxlock::WAIT)?;
    let reload = || store.box_row(id)?.filter(open).ok_or_else(not_open);
    let mut row = reload()?;
    if wake_and_revive(cx, store, &row)? {
        row = reload()?;
    }
    let handle = usable_handle(&row)?;
    let recorded = row.boxed.as_ref().and_then(|b| b.workdir.as_deref());
    let from = match recorded.and_then(recorded_workdir) {
        Some(w) => w,
        None => cx.driver().workdir(&handle)?,
    };
    let stats = cx
        .driver()
        .mirror_out(&handle, &from, &to)
        .map_err(|e| match e {
            DriverError::Unsupported(what) => CommandError::new(
                ErrorKind::Refused,
                format!("box-mirror needs {what}, which is missing"),
            ),
            other => other.into(),
        })?;
    Ok(MirrorValue {
        from: from.as_str().to_owned(),
        to: to.display().to_string(),
        files: stats.files,
        bytes: stats.bytes,
        confirmation: format!(
            "mirrored {} to {} ({} files, {} bytes sent)",
            from.as_str(),
            to.display(),
            stats.files,
            stats.bytes
        ),
    })
}

#[cfg(test)]
mod tests;
