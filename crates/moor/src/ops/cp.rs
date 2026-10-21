//! `cp`: copy a file or directory out of a Box's machine to the host.
//!
//! Holds the Box lock for the copy and never syncs the worktree.

use std::fs;
use std::path::Path;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::view::run::CpValue;
use moor_core::{CpReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::host_path::absolute;
use crate::ops::common::wake::wake_and_revive;
use crate::store::BoxRow;

/// Run `cp`.
pub(crate) fn run(cx: &Cx, req: &CpReq) -> Result<CpValue, CommandError> {
    if req.src.trim().is_empty() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "the source path is empty",
        ));
    }
    let to = absolute("the destination", &req.dest)?;
    let store = cx.store()?;
    let id = store
        .open_box_by_name(&req.box_name)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open Box {}", req.box_name.as_str()),
            )
        })?
        .id;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "cp", boxlock::WAIT)?;
    let reload = || {
        store.box_row(id)?.ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no Box {}", req.box_name.as_str()),
            )
        })
    };
    let mut row = reload()?;
    if wake_and_revive(cx, store, &row)? {
        row = reload()?;
    }
    let handle = usable_handle(&row)?;
    let from = source(cx, &handle, &req.src)?;
    let copied = cx.driver().copy_out(&handle, &from, &to);
    let event = ExtraEvent::Copied {
        from: from.as_str().to_owned(),
        to: to.display().to_string(),
    };
    store.apply(Decision::new(id).event(event))?;
    copied?;
    let bytes = size(&to);
    Ok(CpValue {
        copied: true,
        from: from.as_str().to_owned(),
        to: to.display().to_string(),
        bytes,
        confirmation: format!(
            "copied {} to {} ({bytes} bytes)",
            from.as_str(),
            to.display()
        ),
    })
}

/// The handle of the Box's ready machine, or a refusal.
fn usable_handle(row: &BoxRow) -> Result<BoxHandle, CommandError> {
    let refuse = |why: &str| CommandError::new(ErrorKind::Refused, why.to_owned());
    if row.lifecycle == BoxLifecycle::WaitingForBox {
        return Err(refuse("the Box is waiting for a machine"));
    }
    let b = row
        .boxed
        .as_ref()
        .ok_or_else(|| refuse("the Box has no machine"))?;
    match b.state {
        BoxState::Ready => {}
        BoxState::Gone | BoxState::Deleted => return Err(refuse("the Box is gone")),
        _ => return Err(refuse("the Box is not ready")),
    }
    b.handle
        .clone()
        .ok_or_else(|| refuse("the Box has not been acquired"))
}

/// The source in the Box: an absolute path as given, else under the Box workdir.
fn source(cx: &Cx, handle: &BoxHandle, src: &str) -> Result<BoxPath, CommandError> {
    let usage = |e: moor_core::name::NameError| CommandError::new(ErrorKind::Usage, e);
    if let Some(rel) = src.strip_prefix('/') {
        return BoxPath::under("/", &without_cur_dir(rel)).map_err(usage);
    }
    let workdir = cx.driver().workdir(handle)?;
    BoxPath::under(workdir.as_str(), &without_cur_dir(src)).map_err(usage)
}

/// `rel` without its `.` components; a path of only `.` becomes empty and is refused.
fn without_cur_dir(rel: &str) -> String {
    rel.split('/')
        .filter(|c| *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Bytes at `path`: a file's or link's length, or the total under a directory.
fn size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    // A link counts as its own length and is never followed.
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|e| size(&e.path()))
        .sum()
}

#[cfg(test)]
mod tests;
