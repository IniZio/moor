//! Promotes Tasks waiting for a Box into the slots a closed Box freed.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use moor_core::config::{Flags, TaskSettings};
use moor_core::decide::capacity::promote_order;
use moor_core::domain::task::TaskState;
use moor_core::{BoxId, BoxName, ProviderId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::capacity;
use crate::ops::common::facts::settings;
use crate::ops::common::provision::{provision, reread, Entry};
use crate::store::Store;

/// How long `close` waits for a waiter's Box lock before skipping that waiter.
pub(crate) const PROMOTE_WAIT: Duration = Duration::from_secs(1);

/// Promotes the oldest tasks waiting for a Box of `provider` into its free slots.
///
/// Returns the names of the tasks that left waiting with a Box acquired, in promotion
/// order. Promotion is best effort: a task that cannot be promoted, or an error
/// that stops the pass, becomes a warning and the pass ends with what it did.
pub(crate) fn promote_waiting(
    cx: &Cx,
    store: &Store,
    provider: &ProviderId,
    op: &str,
) -> Vec<BoxName> {
    let mut promoted = Vec::new();
    if let Err(e) = promote_into(cx, store, provider, op, &mut promoted) {
        cx.warn(Warning {
            code: "promote_failed".to_owned(),
            message: format!("cannot promote waiting tasks: {}", e.error),
        });
    }
    promoted
}

/// One promotion pass; `promoted` keeps what was promoted before an error.
fn promote_into(
    cx: &Cx,
    store: &Store,
    provider: &ProviderId,
    op: &str,
    promoted: &mut Vec<BoxName>,
) -> Result<(), CommandError> {
    let Some(lim) = capacity::limit(cx.config(), provider.as_str()) else {
        return Ok(());
    };
    let mut cfgs: HashMap<BoxId, (BoxName, TaskSettings)> = HashMap::new();
    for row in store.waiting_tasks()? {
        let at = row.worktree.as_deref().unwrap_or(&row.repo);
        match settings(cx, Path::new(at), &Flags::default()) {
            Ok(cfg) if cfg.provider.value == provider.as_str() => {
                cfgs.insert(row.id, (row.name.clone(), cfg));
            }
            Ok(_) => {}
            Err(e) => cx.warn(Warning {
                code: "promote_skipped".to_owned(),
                message: format!("cannot promote {}: {}", row.name.as_str(), e.error),
            }),
        }
    }
    let held = store
        .box_rows_live()?
        .iter()
        .filter(|(_, b)| b.provider == *provider)
        .count();
    let free = usize::try_from(lim.max.get())
        .unwrap_or(usize::MAX)
        .saturating_sub(held);
    let ids: Vec<BoxId> = cfgs.keys().copied().collect();
    let free = u32::try_from(free).unwrap_or(u32::MAX);
    for id in promote_order(&ids, free) {
        let Some((name, cfg)) = cfgs.get(&id) else {
            continue;
        };
        if let Some(name) = promote_one(cx, store, id, name, op, cfg)? {
            promoted.push(name);
        }
    }
    Ok(())
}

/// Promotes one waiting task under its Box lock; `None` when it stays waiting.
fn promote_one(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    name: &BoxName,
    op: &str,
    cfg: &TaskSettings,
) -> Result<Option<BoxName>, CommandError> {
    let _lock = match BoxLock::acquire(cx.clock(), cx.state_dir()?, id, op, PROMOTE_WAIT) {
        Ok(lock) => lock,
        Err(e) => {
            cx.warn(Warning {
                code: "promote_skipped".to_owned(),
                message: format!("cannot promote {}: {}", name.as_str(), e.error),
            });
            return Ok(None);
        }
    };
    let row = reread(store, id)?;
    if row.state != TaskState::WaitingForBox || row.intent.is_some() {
        return Ok(None);
    }
    if let Err(e) = provision(cx, store, id, Entry::Waiting, cfg) {
        cx.warn(Warning {
            code: "promote_failed".to_owned(),
            message: format!("cannot promote {}: {}", row.name.as_str(), e.error),
        });
        return Ok(None);
    }
    let after = reread(store, id)?;
    Ok((after.state != TaskState::WaitingForBox).then_some(after.name))
}
