//! Finds proof files in each open Task's Box, records one `proof` event per new or changed
//! file, and copies each into the state directory.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use moor_core::decide::proof::{
    copy_relpath, fresh, listing_argv, parse_listing, refusal, ProofFile, PROOF_DIR,
};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxPath, TaskId, Warning};

use crate::cx::Cx;
use crate::store::{Store, TaskRow};

/// Wall-clock allowance for the listing script.
const LISTING_DEADLINE: Duration = Duration::from_secs(30);

/// Records and copies the fresh proof files of every open Task with a Ready Box.
///
/// A file over `cap` bytes is recorded as refused and not copied. A failure on one Task
/// becomes a warning and the other Tasks still run; a file whose copy failed records
/// nothing, so the next pass tries it again.
pub(crate) fn sync_proofs(cx: &Cx, store: &Store, cap: u64) {
    let Ok(tasks) = store.open_tasks() else {
        return;
    };
    for row in &tasks {
        if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
            continue;
        }
        if let Err(message) = sync_task(cx, store, row, cap) {
            cx.warn(Warning {
                code: "proof_failed".to_owned(),
                message: format!("proof files of task {}: {message}", row.name.as_str()),
            });
        }
    }
}

/// Runs the listing in the Box without syncing and parses what it printed. `at` is the
/// Box workdir and host worktree to run in; a log the script never wrote is an empty listing.
pub(crate) fn list(
    cx: &Cx,
    handle: &BoxHandle,
    id: TaskId,
    at: Option<(&BoxPath, &Path)>,
) -> Result<Vec<ProofFile>, String> {
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let log = state.join(format!("proof-listing-{}.log", id.get()));
    let spec = ExecSpec {
        argv: listing_argv(),
        env: Vec::new(),
        cwd: at.map(|(w, _)| w.clone()),
        sync: SyncMode::None,
        output: Some(Output::Log(log.clone())),
        host_cwd: at.map(|(_, h)| h.to_path_buf()),
        class: None,
    };
    let ran = cx.driver().exec(handle, &spec, Deadline(LISTING_DEADLINE));
    let text = fs::read_to_string(&log);
    let _ = fs::remove_file(&log);
    match ran {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        Ok(ExecOutcome::Exited { code }) => return Err(format!("the listing exited {code}")),
        Ok(ExecOutcome::Interrupted) => return Err("the listing was interrupted".to_owned()),
        Err(e) => return Err(format!("the listing could not run: {e}")),
    }
    let text = match text {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read the listing: {e}")),
    };
    Ok(parse_listing(&text))
}

/// One Task's pass; the error names the first thing that stopped it.
fn sync_task(cx: &Cx, store: &Store, row: &TaskRow, cap: u64) -> Result<(), String> {
    let Some(handle) = row.boxed.as_ref().and_then(|b| b.handle.as_ref()) else {
        return Ok(());
    };
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let workdir = cx
        .driver()
        .workdir(handle, worktree)
        .map_err(|e| format!("cannot find the box workdir: {e}"))?;
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let listing = list(cx, handle, row.id, Some((&workdir, worktree)))?;
    let new = fresh(&listing, &seen(store, row)?);
    for file in &new {
        let refused = refusal(file, cap);
        if refused.is_none() {
            let from = BoxPath::under(workdir.as_str(), &format!("{PROOF_DIR}/{}", file.name))
                .map_err(|e| format!("bad proof path {}: {e}", file.name))?;
            let to = state.join(copy_relpath(row.name.as_str(), &file.name));
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            cx.driver()
                .copy_out(handle, &from, &to, worktree)
                .map_err(|e| format!("cannot copy {}: {e}", file.name))?;
        }
        let event = ExtraEvent::Proof {
            name: file.name.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
            refused,
        };
        store
            .apply(Decision::new(row.id).event(event))
            .map_err(|e| format!("cannot record {}: {e}", file.name))?;
    }
    Ok(())
}

/// The proof files already recorded for the Task, oldest first.
fn seen(store: &Store, row: &TaskRow) -> Result<Vec<ProofFile>, String> {
    let events = store
        .events(row.id, usize::MAX)
        .map_err(|e| format!("cannot read events: {e}"))?;
    Ok(events
        .iter()
        .filter(|e| e.kind == "proof")
        .filter_map(|e| {
            let field = |k: &str| e.payload.get(k);
            Some(ProofFile {
                name: field("name")?.as_str()?.to_owned(),
                size: field("size")?.as_u64()?,
                sha256: field("sha256")?.as_str()?.to_owned(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests;
