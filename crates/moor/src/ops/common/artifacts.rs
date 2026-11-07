//! Copies files the agent published under `.moor/artifacts` out of a Box into the store.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::artifact::{fresh, parse_index, valid_name, ArtifactFile};
use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::{BoxStep, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, Warning};

use self::fetch::Fetch;
use super::boxlock::BoxLock;
use super::provision::recorded_workdir;
use super::reports::{self, target};
use crate::cx::Cx;
use crate::store::{BoxRow, Store};

/// Wall-clock allowance for the index script.
const INDEX_DEADLINE: Duration = Duration::from_secs(60);
/// Artifacts directory, relative to the workdir.
const ARTIFACTS_DIR: &str = ".moor/artifacts";

/// Prints `name<TAB>size<TAB>sha256` for each top-level regular file; symlinks are never listed.
const INDEX_SCRIPT: &str = r#"[ -L .moor ] && exit 0
[ -L .moor/artifacts ] && exit 0
[ -d .moor/artifacts ] || exit 0
cd .moor/artifacts || exit 0
find . -maxdepth 1 -type f ! -name '*
*' | while IFS= read -r f; do
  n=${f#./}
  s=$(wc -c <"$f" | tr -d ' ')
  h=$(sha256sum <"$f" 2>/dev/null | cut -d' ' -f1)
  [ -n "$h" ] || h=$(shasum -a 256 <"$f" 2>/dev/null | cut -d' ' -f1)
  [ -n "$h" ] || continue
  printf '%s\t%s\t%s\n' "$n" "$s" "$h"
done
"#;

/// Collects the artifacts and Reports of each Box whose newest idle or finished turn is past its cursor.
///
/// A Box whose lock an operation holds is skipped and stays due for the next pass. The cursor
/// moves only after a collect. Never errors; a failure warns.
pub(crate) fn collect_due(cx: &Cx, store: &Store, draining: &dyn Fn() -> bool) {
    let Ok(state_dir) = cx.state_dir() else {
        return;
    };
    let due = match store.artifacts_due() {
        Ok(due) => due,
        Err(e) => return failed(cx, &e.to_string()),
    };
    for (id, newest) in due {
        if draining() {
            break;
        }
        let Ok(_lock) = BoxLock::acquire(cx.clock(), state_dir, id, "artifacts", Duration::ZERO)
        else {
            continue;
        };
        let Ok(Some(row)) = store.box_row(id) else {
            continue;
        };
        let ready = row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Ready && b.handle.is_some());
        if !ready || row.intent.is_some() {
            continue;
        }
        let cursor = match store.newest_turn_event(id) {
            Ok(found) => found.unwrap_or(newest),
            Err(e) => return failed(cx, &e.to_string()),
        };
        collect(cx, store, &row);
        reports::collect(cx, store, &row);
        if let Err(e) = store.set_artifacts_through(id, cursor) {
            failed(cx, &e.to_string());
        }
    }
}

/// Warns that the artifact pass could not use the store.
fn failed(cx: &Cx, reason: &str) {
    cx.warn(Warning {
        code: "artifacts_collect_failed".to_owned(),
        message: format!("artifacts were not fully saved: {reason}"),
    });
}

/// Copies new or changed artifacts from a Ready Box into the store; returns how many were stored.
///
/// A failure only warns, so the caller's outcome never changes. File contents never appear in
/// a warning. The caller holds the Box lock.
pub(crate) fn collect(cx: &Cx, store: &Store, row: &BoxRow) -> usize {
    collect_in(cx, store, row, None)
}

/// [`collect`], optionally holding back files not yet stable.
///
/// With `held` (a mid-turn pass), `held` is the previous pass's index. Only files whose name,
/// size and hash match it are stored, since a file the agent is still writing changes between
/// passes. `held` becomes this pass's index.
fn collect_in(cx: &Cx, store: &Store, row: &BoxRow, held: Option<&mut Vec<ArtifactFile>>) -> usize {
    let Some((b, handle)) = target(row) else {
        return 0;
    };
    let mut tally = Tally {
        stored: 0,
        missed: 0,
        held,
        within: None,
    };
    let workdir = match b.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => match cx.driver().workdir(handle) {
            Ok(w) => w,
            Err(_) => return 0,
        },
    };
    if let Err(reason) = run(cx, store, row, (handle, &workdir), &mut tally) {
        cx.warn(Warning {
            code: "artifacts_collect_failed".to_owned(),
            message: format!("artifacts were not fully saved: {reason}"),
        });
    }
    tally.stored
}

/// [`collect`] with `within` for each step in the Box, which fails unless every fresh file is
/// stored. It never falls back to the slower copy of one file at a time.
pub(crate) fn collect_within(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    within: Duration,
) -> Result<(), String> {
    let (b, handle) = target(row).ok_or("the Box is not ready")?;
    let workdir = match b.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => cx
            .driver()
            .workdir(handle)
            .map_err(|e| format!("cannot find the box workdir: {e}"))?,
    };
    let mut tally = Tally {
        stored: 0,
        missed: 0,
        held: None,
        within: Some(within),
    };
    run(cx, store, row, (handle, &workdir), &mut tally)?;
    match tally.missed {
        0 => Ok(()),
        n => Err(format!("{n} artifacts were not saved")),
    }
}

/// What one collect stored and missed, and how it runs.
struct Tally<'a> {
    /// New versions stored.
    stored: usize,
    /// Fresh files not stored.
    missed: usize,
    /// A mid-turn pass's previous index.
    held: Option<&'a mut Vec<ArtifactFile>>,
    /// A limit on each step in the Box, for a collect that must not hold up a suspend.
    within: Option<Duration>,
}

/// Indexes the Box's artifacts and stores each fresh one.
fn run(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    tally: &mut Tally<'_>,
) -> Result<(), String> {
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let staging = private_staging(state, &row.id.get().to_string())?;
    let result = stage_and_store(cx, store, row, (handle, workdir), &staging, tally);
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Runs the index script, then stores what is fresh.
fn stage_and_store(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    staging: &Path,
    tally: &mut Tally<'_>,
) -> Result<(), String> {
    let text = index_text(cx, handle, workdir, tally.within)?;
    let (index, bad) = parse_index(&text);
    if bad > 0 {
        skipped(cx, &format!("{bad} unreadable index lines were skipped"));
    }
    let latest = store
        .latest_artifacts(row.id)
        .map_err(|e| format!("cannot read stored artifacts: {e}"))?
        .into_iter()
        .map(|a| ArtifactFile {
            name: a.name,
            size: a.size,
            sha256: a.sha256,
        })
        .collect::<Vec<_>>();
    let index: Vec<_> = index.into_iter().filter(|f| valid_name(&f.name)).collect();
    let index = match tally.held.as_deref_mut() {
        Some(prev) => {
            let before = std::mem::replace(prev, index.clone());
            index.into_iter().filter(|f| before.contains(f)).collect()
        }
        None => index,
    };
    let fetch = Fetch {
        cx,
        store,
        row,
        handle,
        workdir,
        blobs: cx.blobs().map_err(|e| e.error.to_string())?,
        staging,
        within: tally.within,
    };
    let due = fresh(&index, &latest);
    let wanted = due.len();
    let got = fetch.all(due);
    tally.stored += got;
    tally.missed += wanted.saturating_sub(got);
    Ok(())
}

/// Runs the index script in the Box and returns what it prints.
fn index_text(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    within: Option<Duration>,
) -> Result<String, String> {
    let script = format!(
        "cd -- {} || exit 1\n{INDEX_SCRIPT}",
        quote(workdir.as_str())
    );
    let steps = [BoxStep::Run(script)];
    let done = cx.driver().batch(
        handle,
        &[],
        &steps,
        Deadline(within.unwrap_or(INDEX_DEADLINE)),
    );
    match done.into_iter().next() {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => Ok(stdout),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        })) => Err(format!("the index script exited {code}")),
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        })) => Err("the index script was interrupted".into()),
        Some(Ok(_)) | None => Err("the index script could not run".into()),
        Some(Err(e)) => Err(format!("the index script could not run: {e}")),
    }
}

/// Creates an empty private staging directory under the state dir; refuses symlinked parts.
fn private_staging(state: &Path, id: &str) -> Result<PathBuf, String> {
    let mut cur = state.to_path_buf();
    for part in ["artifacts", ".staging", id] {
        cur.push(part);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "refusing to stage through symlink {}",
                    cur.display()
                ));
            }
            Ok(_) if part == id => {
                discard(&cur);
                make_private(&cur)?;
            }
            Ok(m) if !m.file_type().is_dir() => {
                return Err(format!("{} is not a directory", cur.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => make_private(&cur)?,
            Err(e) => return Err(format!("cannot inspect {}: {e}", cur.display())),
        }
    }
    Ok(cur)
}

/// Creates one directory readable by the owner only.
fn make_private(path: &Path) -> Result<(), String> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))
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

/// Warns that one artifact was left out.
fn skipped(cx: &Cx, message: &str) {
    cx.warn(Warning {
        code: "artifact_skipped".to_owned(),
        message: message.to_owned(),
    });
}

mod fetch;
mod midturn;
pub(crate) use midturn::collect_working;
#[cfg(test)]
mod tests;
