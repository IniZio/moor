//! Clean-up of the folders closed Boxes leave under the account's home.
//!
//! The runner names the Boxes still open; every other `moor/<id>` and `moor-state/<id>` goes,
//! after that Box's `release` hook. Nothing else is touched: only real directories named for a
//! plain id count, and removal never follows a symlink out of the two folders.
//!
//! The runner lists the open Boxes before this machine looks at its folders, so a Box opened in
//! between has folders yet is missing from the list. A Box has folders only once it is open, so
//! folders older than [`GRACE`] belong to a Box that was open before the listing, and any younger
//! wait for a later sweep. Age is the status-change time on this machine's own clock: rsync
//! restores a copy's modification time, never its change time.

use std::collections::BTreeSet;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use moor_client::link::ops::{Hook, Reply};
use moor_client::link::status::clip;
use moor_client::link::sweep::{Failed, Swept};
use moor_client::link::{ops, Channel};
use moor_core::box_key::{BoxKey, STATE_ROOT, WORK_ROOT};
use moor_core::BoxId;

use crate::serve::{self, Machine};

/// How old a Box's folders must be before a sweep removes them; far longer than the seconds
/// between the runner's listing and this machine's look, and short beside a closed Box's disk.
pub(crate) const GRACE: Duration = Duration::from_secs(3600);
/// How long a Box's `release` hook may run before it is killed and the Box is kept.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(600);
/// Most chars of a hook's last output line a failure quotes.
const QUOTED: usize = 120;

/// Sweeps the Boxes not in `keep` and answers what came of it; why any failed.
pub(crate) async fn serve(machine: &Machine, ch: Channel, keep: &[i64]) -> Option<String> {
    let swept = sweep(machine, keep).await;
    let failed = swept
        .errors
        .first()
        .map(|e| format!("Box {}: {}", e.box_id, e.reason));
    serve::end(ch, Reply::Swept(swept)).await;
    failed
}

/// Removes Box `box_id`'s folders, its `release` having run already, and answers what came of
/// it; why it failed.
pub(crate) async fn serve_remove(machine: &Machine, ch: Channel, box_id: i64) -> Option<String> {
    let mut swept = Swept::default();
    let result = match BoxKey::from_dir_name(&box_id.to_string()) {
        Some(key) => remove(&machine.home, key).await,
        None => Err("not a Box id".to_owned()),
    };
    let failed = match result {
        Ok(()) => {
            swept.removed.push(box_id);
            None
        }
        Err(reason) => {
            swept.errors.push(Failed {
                box_id,
                reason: reason.clone(),
            });
            Some(reason)
        }
    };
    serve::end(ch, Reply::Swept(swept)).await;
    failed
}

/// Cleans up each Box that has folders older than the grace and is not in `keep`.
async fn sweep(machine: &Machine, keep: &[i64]) -> Swept {
    let mut swept = Swept::default();
    for id in closed(&machine.home, keep, machine.sweep_grace).await {
        match clean(machine, id).await {
            Ok(()) => swept.removed.push(id),
            Err(reason) => swept.errors.push(Failed { box_id: id, reason }),
        }
    }
    swept
}

/// The ids with a folder under `home` that are not in `keep`, less any with a folder younger
/// than `grace`.
async fn closed(home: &Path, keep: &[i64], grace: Duration) -> BTreeSet<i64> {
    let since =
        crate::unix_now().saturating_sub(i64::try_from(grace.as_secs()).unwrap_or(i64::MAX));
    let (mut ids, mut young) = (BTreeSet::new(), BTreeSet::new());
    for root in [WORK_ROOT, STATE_ROOT] {
        for (id, changed) in box_dirs(&home.join(root)).await {
            ids.insert(id);
            if changed > since {
                young.insert(id);
            }
        }
    }
    ids.retain(|id| !keep.contains(id) && !young.contains(id));
    ids
}

/// Whether `root` is a real directory, not a symlink to one.
async fn real_dir(root: &Path) -> bool {
    tokio::fs::symlink_metadata(root)
        .await
        .is_ok_and(|m| m.is_dir())
}

/// The ids of the directories directly in `root`, each with its status-change time; a symlink, a
/// file, a name that is not a plain id, or a `root` that is itself a symlink counts for nothing.
async fn box_dirs(root: &Path) -> Vec<(i64, i64)> {
    let entries = if real_dir(root).await {
        tokio::fs::read_dir(root).await
    } else {
        Err(io::ErrorKind::NotFound.into())
    };
    let Ok(mut entries) = entries else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(key) = entry.file_name().to_str().and_then(BoxKey::from_dir_name) else {
            continue;
        };
        if let Ok(meta) = tokio::fs::symlink_metadata(entry.path()).await {
            if meta.is_dir() {
                ids.push((key.id().get(), meta.ctime()));
            }
        }
    }
    ids
}

/// Runs Box `id`'s `release` hook if the machine has one, then removes its folders. A hook that
/// fails keeps them, so the next sweep tries again.
async fn clean(machine: &Machine, id: i64) -> Result<(), String> {
    let env = [("MOOR_BOX".to_owned(), id.to_string())];
    if let Some(script) = serve::hook_script(machine, id, Hook::Release, &env) {
        let ran = ops::run(machine.carrier(), &script, RELEASE_TIMEOUT)
            .await
            .map_err(|e| format!("cannot run the shell: {e}"))?;
        if ran.timed_out {
            return Err("release timed out".to_owned());
        }
        if ran.exit_code != Some(0) {
            let last = ran.output.lines().last().unwrap_or_default();
            return Err(format!(
                "release exited {:?}: {}",
                ran.exit_code,
                clip(last, QUOTED)
            ));
        }
    }
    remove(&machine.home, BoxKey::new(BoxId::new(id))).await
}

/// Removes `key`'s two folders under `home`, never through a root that is a symlink.
async fn remove(home: &Path, key: BoxKey) -> Result<(), String> {
    for root in [WORK_ROOT, STATE_ROOT] {
        let path = home.join(root);
        if tokio::fs::symlink_metadata(&path).await.is_ok() && !real_dir(&path).await {
            return Err(format!("{root} is not a folder"));
        }
    }
    for dir in [key.machine_dir(), key.machine_state_dir()] {
        match tokio::fs::remove_dir_all(home.join(&dir)).await {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(format!("cannot remove {dir}: {e}"));
            }
            _ => {}
        }
    }
    Ok(())
}
