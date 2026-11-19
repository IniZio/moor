//! The pull requests the `pulls` request read lately, one record per controller.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::name::BoxId;
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::{BranchPull, ForgeRepo};

/// How long a read is answered again before the forge is asked anew.
pub(crate) const FRESH: Duration = Duration::from_secs(30);

/// How many reads the record holds before a put drops those past [`FRESH`].
const PRUNE_AT: usize = 256;

/// One read: the branch and its pull request, or why the forge could not be read.
pub(crate) type PullRead = Result<BranchPull, String>;

/// A Box and one of its repos; keyed by Box, so one person's read never answers another's.
pub(crate) type Key = (BoxId, ForgeRepo);

/// The reads of the last [`FRESH`], by Box and repo.
#[derive(Default)]
pub(crate) struct PullCache {
    /// When each read was made, and what it read.
    held: Mutex<HashMap<Key, (Ts, PullRead)>>,
}

impl PullCache {
    /// The read of `key` made within [`FRESH`] of `now`.
    pub(crate) fn get(&self, key: &Key, now: Ts) -> Option<PullRead> {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .filter(|(at, _)| now < at.saturating_add(FRESH))
            .map(|(_, read)| read.clone())
    }

    /// Records `read` of `key`, made at `now`.
    pub(crate) fn put(&self, key: Key, now: Ts, read: PullRead) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.len() >= PRUNE_AT {
            held.retain(|_, (at, _)| now < at.saturating_add(FRESH));
        }
        held.insert(key, (now, read));
    }
}
