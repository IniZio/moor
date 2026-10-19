//! Controller start-up: keep live Proxy supervisors, restart dead ones, drop orphans.

use std::path::Path;
use std::sync::Arc;
use std::thread;

use moor_core::domain::boxes::BoxState;
use moor_core::seam::clock::Clock;
use moor_core::seam::proc::Proc;
use moor_core::seam::proxy_host::ProxyHost;
use moor_core::BoxId;

use super::detached::{read_spec, DetachedSupervisor};
use crate::store::{ProxyRow, Store, STORE_FILE};

/// What the controller does with one recorded Proxy.
enum Fate {
    /// The supervisor is ours and healthy: leave it running.
    Keep,
    /// The Box needs its Proxy but the supervisor is gone: start another on the same port.
    Restart,
    /// The Box no longer needs a Proxy.
    Stop,
}

/// Walks every recorded Proxy and settles it; returns one line per failure.
///
/// A row is kept only when its pid is alive, leads its own process group, belongs to the
/// current boot and holds a live lease: a pid alone may have been reused. Any other row of a
/// A Box with a ready machine is restarted from the spec saved at its last `ensure`, keeping its box port.
/// Rows of Boxes without a usable machine are stopped. A failure for one Box never stops the rest.
pub(crate) fn reconcile(
    host: &DetachedSupervisor,
    proc: &dyn Proc,
    clock: Arc<dyn Clock>,
    state_dir: &Path,
) -> Vec<String> {
    let store = match Store::open(&state_dir.join(STORE_FILE), clock) {
        Ok(store) => store,
        Err(e) => return vec![format!("cannot read the proxy state: {e}")],
    };
    let (rows, live) = match (store.proxies(), store.open_boxes()) {
        (Ok(rows), Ok(live)) => (rows, live),
        (Err(e), _) | (_, Err(e)) => return vec![format!("cannot read the proxy state: {e}")],
    };
    let boxes: Vec<(BoxId, BoxState)> = live
        .iter()
        .filter_map(|t| Some((t.id, t.boxed.as_ref()?.state)))
        .collect();
    thread::scope(|scope| {
        let jobs: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                let state = boxes
                    .iter()
                    .find(|(t, _)| *t == row.box_id)
                    .map(|(_, s)| *s);
                let fate = fate(host, proc, row, state)?;
                let store = &store;
                Some(scope.spawn(move || {
                    settle(host, store, state_dir, row.box_id, &fate)
                        .err()
                        .map(|e| format!("Box {}: {e}", row.box_id.get()))
                }))
            })
            .collect();
        jobs.into_iter()
            .filter_map(|j| j.join().ok().flatten())
            .collect()
    })
}

/// The fate of `row`, or `None` when the Box is asleep and its Proxy is left alone.
fn fate(
    host: &DetachedSupervisor,
    proc: &dyn Proc,
    row: &ProxyRow,
    state: Option<BoxState>,
) -> Option<Fate> {
    match state {
        Some(BoxState::Starting | BoxState::Ready) => {}
        Some(BoxState::Asleep) => return None,
        _ => return Some(Fate::Stop),
    }
    // Pids 0 and 1 are never a supervisor.
    let ours = row.ready
        && row
            .pid
            .is_some_and(|p| p > 1 && proc.alive(p) && proc.group_leader(p))
        && host.same_boot(row)
        && host.live(row);
    Some(if ours { Fate::Keep } else { Fate::Restart })
}

/// Carries out `fate` for `box_id`.
fn settle(
    host: &DetachedSupervisor,
    store: &Store,
    state_dir: &Path,
    box_id: BoxId,
    fate: &Fate,
) -> Result<(), String> {
    match fate {
        Fate::Keep => Ok(()),
        Fate::Stop => host.stop(box_id).map_err(|e| e.to_string()),
        Fate::Restart => {
            // Without the saved spec the row stays expired, so the next `ensure` restarts it.
            store.expire_proxy(box_id).map_err(|e| e.to_string())?;
            let spec = read_spec(state_dir, box_id)
                .map_err(|e| format!("cannot restart the proxy without its spec: {e}"))?;
            host.ensure(&spec).map(drop).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests;
