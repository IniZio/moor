//! Copies files the agent published under `.moor/artifacts` out of a Box into the store.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::artifact::{fresh, parse_index, valid_name, ArtifactFile};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxId, BoxPath, Warning};
use sha2::{Digest, Sha256};

use super::boxlock::BoxLock;
use super::provision::recorded_workdir;
use super::reports::{host_of, target};
use crate::cx::Cx;
use crate::store::{Store, TaskRow};

/// Wall-clock allowance for the index script.
const INDEX_DEADLINE: Duration = Duration::from_secs(60);
/// Artifacts directory, relative to the workdir.
const ARTIFACTS_DIR: &str = ".moor/artifacts";
/// Index output file name inside the staging directory; not a valid artifact name.
const INDEX_LOG: &str = ".index";

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

/// Collects the artifacts of each Box in `due` that is Ready and idle of any operation.
///
/// A Box whose lock an operation holds is skipped; the next pass or the close collects it.
/// Never errors; a failure warns.
pub(crate) fn collect_due(cx: &Cx, store: &Store, due: &[BoxId]) {
    let Ok(state_dir) = cx.state_dir() else {
        return;
    };
    for &id in due {
        let Ok(_lock) = BoxLock::acquire(cx.clock(), state_dir, id, "artifacts", Duration::ZERO)
        else {
            continue;
        };
        let Ok(Some(row)) = store.task(id) else {
            continue;
        };
        let ready = row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Ready && b.handle.is_some());
        if ready && row.intent.is_none() {
            collect(cx, store, &row);
        }
    }
}

/// Copies new or changed artifacts from a Ready Box into the store; returns how many were stored.
///
/// A failure only warns, so the caller's outcome never changes. File contents never appear in
/// a warning. The caller holds the Box lock.
pub(crate) fn collect(cx: &Cx, store: &Store, row: &TaskRow) -> usize {
    let Some((b, handle)) = target(row) else {
        return 0;
    };
    let host = host_of(row);
    let workdir = match b.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => match cx.driver().workdir(handle, host) {
            Ok(w) => w,
            Err(_) => return 0,
        },
    };
    let mut stored = 0;
    if let Err(reason) = run(cx, store, row, (handle, &workdir), &mut stored) {
        cx.warn(Warning {
            code: "artifacts_collect_failed".to_owned(),
            message: format!("artifacts were not fully saved: {reason}"),
        });
    }
    stored
}

/// Indexes the Box's artifacts and stores each fresh one.
fn run(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    stored: &mut usize,
) -> Result<(), String> {
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let dir = state.join("artifacts");
    let staging = private_staging(state, &row.id.get().to_string())?;
    let result = stage_and_store(cx, store, row, (handle, workdir), (&dir, &staging), stored);
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Runs the index script, then stores what is fresh.
fn stage_and_store(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    (dir, staging): (&Path, &Path),
    stored: &mut usize,
) -> Result<(), String> {
    let host = host_of(row);
    let log = staging.join(INDEX_LOG);
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), INDEX_SCRIPT.to_owned()],
        env: Vec::new(),
        cwd: Some(workdir.clone()),
        sync: SyncMode::None,
        output: Some(Output::Log(log.clone())),
        host_cwd: Some(host.to_path_buf()),
        class: None,
    };
    match cx.driver().exec(handle, &spec, Deadline(INDEX_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        Ok(ExecOutcome::Exited { code }) => return Err(format!("the index script exited {code}")),
        Ok(ExecOutcome::Interrupted) => return Err("the index script was interrupted".into()),
        Err(e) => return Err(format!("the index script could not run: {e}")),
    }
    let text = fs::read(&log).map_err(|e| format!("cannot read the index: {e}"))?;
    let (index, bad) = parse_index(&String::from_utf8_lossy(&text));
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
    for file in fresh(&index, &latest) {
        if store_one(cx, store, row, (handle, workdir), (dir, staging), &file) {
            *stored += 1;
        }
    }
    Ok(())
}

/// Copies one file out of the Box and stores it; true when a new version was stored.
fn store_one(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    (dir, staging): (&Path, &Path),
    file: &ArtifactFile,
) -> bool {
    let name = &file.name;
    let Ok(from) = BoxPath::under(workdir.as_str(), &format!("{ARTIFACTS_DIR}/{name}")) else {
        skipped(
            cx,
            &format!("artifact {name} has an unusable path and was skipped"),
        );
        return false;
    };
    let staged = staging.join(name);
    discard(&staged);
    if let Err(e) = cx.driver().copy_out(handle, &from, &staged, host_of(row)) {
        skipped(
            cx,
            &format!("artifact {name} could not be copied: {e}; it will be tried next time"),
        );
        discard(&staged);
        return false;
    }
    let bytes = if is_regular_file(&staged) {
        fs::read(&staged).ok()
    } else {
        None
    };
    discard(&staged);
    let Some(bytes) = bytes else {
        skipped(
            cx,
            &format!("artifact {name} was not a plain file when copied and was skipped"),
        );
        return false;
    };
    if bytes.len() as u64 != file.size || format!("{:x}", Sha256::digest(&bytes)) != file.sha256 {
        skipped(
            cx,
            &format!("artifact {name} changed while it was copied; it will be collected next time"),
        );
        return false;
    }
    match store.add_artifact(row.id, name, &bytes, dir) {
        Ok(stored) => stored.is_some(),
        Err(e) => {
            skipped(cx, &format!("artifact {name} could not be stored: {e}"));
            false
        }
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

#[cfg(test)]
mod tests;
