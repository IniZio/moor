//! Finds proof files in each open Task's Box, records one `proof` event per new or changed
//! file, and copies each into the state directory.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::decide::proof::{
    copy_relpath, fresh, listing_argv, parse_listing, refusal, ProofFile, PROOF_DIR,
};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{BoxStep, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, BoxPath, Warning};

use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::provision::recorded_workdir;
use crate::store::{Store, TaskRow};

/// Wall-clock allowance for the listing script.
const LISTING_DEADLINE: Duration = Duration::from_secs(30);

/// How long a Box is left alone after a failed pass.
const BACKOFF: Duration = Duration::from_secs(300);

/// When each failing Box may be probed again.
#[derive(Debug, Default)]
pub(crate) struct ProofBackoff {
    /// Earliest next probe per Box.
    until: Mutex<HashMap<BoxId, Ts>>,
}

impl ProofBackoff {
    /// The locked table.
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<BoxId, Ts>> {
        self.until.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// True while `id` is inside its backoff window.
    fn waiting(&self, id: BoxId, now: Ts) -> bool {
        self.map().get(&id).is_some_and(|until| now < *until)
    }

    /// Starts the window for `id`.
    fn failed(&self, id: BoxId, now: Ts) {
        self.map().insert(id, now.saturating_add(BACKOFF));
    }

    /// Ends the window for `id`.
    fn cleared(&self, id: BoxId) {
        self.map().remove(&id);
    }
}

/// Records and copies the fresh proof files of every open Task with a Ready Box.
///
/// A file over `cap` bytes is recorded as refused and not copied. A failure on one Task
/// becomes a warning and the other Tasks still run; a file whose copy failed records
/// nothing, so the next pass tries it again. The Box lock is never held across a Box call,
/// only across the local writes, and a failed Task is left alone for five minutes.
pub(crate) fn sync_proofs(cx: &Cx, store: &Store, cap: u64, backoff: &ProofBackoff) {
    let Ok(tasks) = store.open_tasks() else {
        return;
    };
    for row in &tasks {
        if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
            continue;
        }
        // A Box is Ready before setup runs; probing it then would hold the provider's
        // workspace lock against setup.
        if row.intent.is_some() {
            continue;
        }
        let Ok(state) = cx.state_dir() else {
            continue;
        };
        if backoff.waiting(row.id, cx.clock().now()) {
            continue;
        }
        if lock(cx, state, row.id).is_none() {
            continue;
        }
        match sync_task(cx, store, row, cap, state) {
            Ok(()) => backoff.cleared(row.id),
            Err(message) => {
                backoff.failed(row.id, cx.clock().now());
                cx.warn(Warning {
                    code: "proof_failed".to_owned(),
                    message: format!("proof files of task {}: {message}", row.name.as_str()),
                });
            }
        }
    }
}

/// Takes the Box lock without waiting; `None` when an operation holds it.
fn lock(cx: &Cx, state: &Path, id: BoxId) -> Option<BoxLock> {
    BoxLock::acquire(cx.clock(), state, id, "proof", Duration::ZERO).ok()
}

/// Runs the listing as one ssh `batch` step in `workdir` and parses its stdout. It never
/// runs `crabbox run`, so it never syncs or queues on the provider's workspace owner.
pub(crate) fn list_batched(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    worktree: &Path,
) -> Result<Vec<ProofFile>, String> {
    let script = listing_argv().pop().unwrap_or_default();
    // A batched `Run` has no working directory, so the script changes into the workdir itself.
    let step = BoxStep::Run(format!(
        "cd {} || exit 1\n{script}",
        quote(workdir.as_str())
    ));
    let done = cx
        .driver()
        .batch(handle, &[], &[step], worktree, Deadline(LISTING_DEADLINE))
        .into_iter()
        .next();
    match done {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => Ok(parse_listing(&stdout)),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => Err(format!("the listing exited {code}")),
        Some(Ok(StepDone::Ran { .. })) => Err("the listing was interrupted".to_owned()),
        Some(Ok(_)) | None => Err("the listing did not run".to_owned()),
        Some(Err(e)) => Err(format!("the listing could not run: {e}")),
    }
}

/// One Task's pass; the error names the first thing that stopped it.
fn sync_task(cx: &Cx, store: &Store, row: &TaskRow, cap: u64, state: &Path) -> Result<(), String> {
    let Some(handle) = row.boxed.as_ref().and_then(|b| b.handle.as_ref()) else {
        return Ok(());
    };
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let stored = row
        .boxed
        .as_ref()
        .and_then(|b| b.workdir.as_deref())
        .and_then(recorded_workdir);
    let workdir = match stored {
        Some(w) => w,
        None => cx
            .driver()
            .workdir(handle, worktree)
            .map_err(|e| format!("cannot find the box workdir: {e}"))?,
    };
    let listing = list_batched(cx, handle, &workdir, worktree)?;
    let new = fresh(&listing, &seen(store, row)?);
    let staging = state.join("proof-staging").join(row.id.get().to_string());
    let result = record_new(
        cx,
        store,
        row,
        cap,
        &new,
        &Ctx {
            handle,
            workdir: &workdir,
            worktree,
            state,
            staging: &staging,
        },
    );
    discard(&staging);
    result
}

/// What `record_new` needs to copy and record files of one Task.
struct Ctx<'a> {
    /// The Box.
    handle: &'a BoxHandle,
    /// Its workdir.
    workdir: &'a BoxPath,
    /// Host worktree.
    worktree: &'a Path,
    /// State directory.
    state: &'a Path,
    /// Where copies land before the lock is taken.
    staging: &'a Path,
}

/// Copies each new file into staging without the Box lock, then moves it into place and
/// records it under the lock. A busy lock drops the staged copy and ends the pass quietly.
fn record_new(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    cap: u64,
    new: &[ProofFile],
    at: &Ctx<'_>,
) -> Result<(), String> {
    let proof_root = at.state.join("proof");
    for file in new {
        let refused = refusal(file, cap);
        let to = at.state.join(copy_relpath(row.id, &file.name));
        let staged = at.staging.join(&file.name);
        if refused.is_none() {
            let from = BoxPath::under(at.workdir.as_str(), &format!("{PROOF_DIR}/{}", file.name))
                .map_err(|e| format!("bad proof path {}: {e}", file.name))?;
            if let Some(link) = symlink_on_path(&proof_root, &to)? {
                warn_symlink(cx, &file.name, &link);
                continue;
            }
            if let Some(dir) = staged.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            discard(&staged);
            cx.driver()
                .copy_out(at.handle, &from, &staged, at.worktree)
                .map_err(|e| format!("cannot copy {}: {e}", file.name))?;
        }
        let Some(_held) = lock(cx, at.state, row.id) else {
            return Ok(());
        };
        if refused.is_none() && !place(cx, &proof_root, &staged, &to, &file.name)? {
            continue;
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

/// Moves the staged copy to its final path; false, with a warning, when it must not be kept.
fn place(cx: &Cx, proof_root: &Path, staged: &Path, to: &Path, name: &str) -> Result<bool, String> {
    if !is_regular_file(staged) {
        discard(staged);
        cx.warn(Warning {
            code: "proof_failed".to_owned(),
            message: format!("proof file {name} not kept: the copy is not a regular file"),
        });
        return Ok(false);
    }
    if let Some(link) = symlink_on_path(proof_root, to)? {
        discard(staged);
        warn_symlink(cx, name, &link);
        return Ok(false);
    }
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::rename(staged, to).map_err(|e| format!("cannot move {name} into place: {e}"))?;
    if !is_regular_file(to) {
        discard(to);
        cx.warn(Warning {
            code: "proof_failed".to_owned(),
            message: format!("proof file {name} not kept: the copy is not a regular file"),
        });
        return Ok(false);
    }
    Ok(true)
}

/// Warns that `link` on the way to `name`'s copy is a symlink.
fn warn_symlink(cx: &Cx, name: &str, link: &Path) {
    cx.warn(Warning {
        code: "proof_failed".to_owned(),
        message: format!(
            "proof file {name} not copied: {} is a symlink",
            link.display()
        ),
    });
}

/// The first symlink among `root` and every existing path component from there down to `to`.
fn symlink_on_path(root: &Path, to: &Path) -> Result<Option<PathBuf>, String> {
    let rest = to.strip_prefix(root).unwrap_or(to);
    let mut cur = root.to_path_buf();
    let steps = std::iter::once(None).chain(rest.components().map(Some));
    for step in steps {
        if let Some(c) = step {
            cur.push(c);
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Ok(Some(cur)),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot inspect {}: {e}", cur.display())),
        }
    }
    Ok(None)
}

/// True when `path` itself, not what it points to, is a regular file.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// Removes whatever sits at `path`, without following a symlink there.
fn discard(path: &Path) {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
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
