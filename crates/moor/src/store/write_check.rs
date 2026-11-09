//! The test-build check run on every write to an existing Box's rows.

#[cfg(test)]
use std::sync::Arc;

use moor_core::BoxId;

use super::Store;

/// Panics on a write to Box rows that nothing may make now.
#[cfg(test)]
pub(crate) type WriteCheck = Arc<dyn Fn(BoxId) + Send + Sync>;

impl Store {
    /// Makes every write to an existing Box's rows, from this store or any clone, pass `check`
    /// first. The first check set stays.
    #[cfg(test)]
    pub(crate) fn check_writes(&self, check: WriteCheck) {
        let _ = self.write_check.set(check);
    }

    /// Runs the write check on Box `id`, if one is set.
    #[cfg(test)]
    pub(super) fn check_write(&self, id: BoxId) {
        if let Some(check) = self.write_check.get() {
            check(id);
        }
    }

    /// Writes are checked in test builds only.
    #[cfg(not(test))]
    #[expect(clippy::unused_self, reason = "the check exists in test builds")]
    pub(super) const fn check_write(&self, _id: BoxId) {}
}
