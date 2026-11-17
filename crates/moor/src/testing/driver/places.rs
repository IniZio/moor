//! What the fake saw of each command's place.

use moor_core::seam::driver::Place;

use super::FakeDriver;

impl FakeDriver {
    /// The place of each call to the Driver op `op` (`exec`, `exec_piped`, `launch_run`,
    /// `peek_file`, `put_file`, `copy_out`, `connect_argv` or `batch`), in call order.
    pub(crate) fn places(&self, op: &str) -> Vec<Place> {
        let s = self.st();
        s.placed
            .iter()
            .filter(|(name, _)| *name == op)
            .map(|(_, place)| *place)
            .collect()
    }
}

impl FakeDriver {
    /// True when the fake saw at least one Box command and every one was given `place`.
    pub(crate) fn all_in(&self, place: Place) -> bool {
        let s = self.st();
        !s.placed.is_empty() && s.placed.iter().all(|(_, p)| *p == place)
    }

    /// True when the fake saw at least one call of `op` and every one was given `place`.
    pub(crate) fn op_in(&self, op: &str, place: Place) -> bool {
        let seen = self.places(op);
        !seen.is_empty() && seen.iter().all(|p| *p == place)
    }
}

#[cfg(test)]
mod tests;
