//! Collects the artifacts of a Box whose agent is still mid-turn, so a human sees them as they appear.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::artifact::ArtifactFile;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use super::collect_in;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
use crate::store::Store;

/// Least time between two mid-turn collects of one Box; also the gap a file must hold still across.
const INTERVAL: Duration = Duration::from_secs(30);

/// When each working Box was last collected and what its index held, per state dir.
///
/// A controller rebuilds its context each pass, so this outlives one. A Box leaves the map once
/// it is no longer working, so its next turn starts fresh.
static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Last collect time and index of each (state dir, Box).
type Seen = HashMap<Key, (Ts, Vec<ArtifactFile>)>;

/// A Box in one state dir.
type Key = (PathBuf, BoxId);

/// Collects each working Box not collected in the last [`INTERVAL`]; never errors.
///
/// Boxes whose lock an operation holds or that have an intent are skipped. A file is stored once
/// two passes saw it with the same size and hash, so one still being written is never stored half
/// done. The turn-end collect still runs separately.
pub(crate) fn collect_working(cx: &Cx, store: &Store, draining: &dyn Fn() -> bool) {
    let Ok(state_dir) = cx.state_dir() else {
        return;
    };
    let Ok(ids) = store.artifacts_ready() else {
        return;
    };
    for id in ids {
        if draining() {
            break;
        }
        let key = (state_dir.to_path_buf(), id);
        let Ok(Some(row)) = store.box_row(id) else {
            continue;
        };
        if row.lifecycle != BoxLifecycle::Ready || row.session.is_quiet() {
            forget(&key);
            continue;
        }
        let now = cx.clock().now();
        let Some(mut held) = due_again(&key, now) else {
            continue;
        };
        let Ok(_lock) = BoxLock::acquire(cx.clock(), state_dir, id, "artifacts", Duration::ZERO)
        else {
            continue;
        };
        collect_in(cx, store, &row, Some(&mut held));
        remember(key, now, held);
    }
}

/// The index held from the last collect, or none when that was less than [`INTERVAL`] ago.
fn due_again(key: &Key, now: Ts) -> Option<Vec<ArtifactFile>> {
    let seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    match seen.as_ref().and_then(|m| m.get(key)) {
        None => Some(Vec::new()),
        Some((at, index)) => {
            let waited = now.as_millis().saturating_sub(at.as_millis());
            (u128::try_from(waited).unwrap_or(0) >= INTERVAL.as_millis()).then(|| index.clone())
        }
    }
}

/// Records that the Box was collected at `at` holding `index`.
fn remember(key: Key, at: Ts, index: Vec<ArtifactFile>) {
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    seen.get_or_insert_with(HashMap::new)
        .insert(key, (at, index));
}

/// Drops what is held for a Box.
fn forget(key: &Key) {
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(m) = seen.as_mut() {
        m.remove(key);
    }
}
