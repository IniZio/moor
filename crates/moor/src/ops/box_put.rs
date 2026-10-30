//! `box-put`: write one host file into a Box's work tree.
//!
//! Holds the Box lock for the write, as `box-mirror` does for its copy.

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::name::BoxPath;
use moor_core::view::run::PutValue;
use moor_core::{BoxPutReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::host_path::{absolute, open_beneath};
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::wake::wake_and_revive;
use crate::ops::cp::usable_handle;

/// The largest file `box-put` takes. The bytes travel in one driver call.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// The mode the file lands with.
const MODE: u32 = 0o644;

/// A refusal of the request as sent.
fn usage(why: String) -> CommandError {
    CommandError::new(ErrorKind::Usage, why)
}

/// Run `box-put`.
pub(crate) fn run(cx: &Cx, req: &BoxPutReq) -> Result<PutValue, CommandError> {
    BoxPath::under("/", &req.dest).map_err(|_| {
        usage(format!(
            "the destination {:?} must be a path inside the work tree: not empty, not absolute, no `..`",
            req.dest
        ))
    })?;
    let bytes = read_source(cx, req)?;

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
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "box-put", boxlock::WAIT)?;
    let reload = || store.box_row(id)?.filter(open).ok_or_else(not_open);
    let mut row = reload()?;
    if wake_and_revive(cx, store, &row)? {
        row = reload()?;
    }
    let handle = usable_handle(&row)?;
    let recorded = row.boxed.as_ref().and_then(|b| b.workdir.as_deref());
    let workdir = match recorded.and_then(recorded_workdir) {
        Some(w) => w,
        None => cx.driver().workdir(&handle)?,
    };
    let to = BoxPath::under(workdir.as_str(), &req.dest).map_err(|e| usage(e.to_string()))?;
    cx.driver().put_file(&handle, &to, &bytes, MODE)?;
    let n = bytes.len() as u64;
    Ok(PutValue {
        path: to.as_str().to_owned(),
        bytes: n,
        confirmation: format!("wrote {n} bytes to {}", to.as_str()),
    })
}

/// The bytes of the request's source, opened beneath the host directory the request is held
/// to when it has one.
fn read_source(cx: &Cx, req: &BoxPutReq) -> Result<Vec<u8>, CommandError> {
    let src = absolute("the source", &req.src)?;
    let cannot =
        |e: std::io::Error| usage(format!("cannot read the source {}: {e}", src.display()));
    let mut file = match cx.host_dir() {
        Some(root) => open_beneath("the source", &req.src, root)?,
        None => std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
            .open(&src)
            .map_err(cannot)?,
    };
    let meta = file.metadata().map_err(cannot)?;
    if !meta.is_file() {
        return Err(usage(format!(
            "the source {} is not a regular file",
            src.display()
        )));
    }
    if meta.len() > MAX_BYTES {
        return Err(usage(format!(
            "the source {} is {} bytes, over the {MAX_BYTES} byte limit",
            src.display(),
            meta.len()
        )));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
