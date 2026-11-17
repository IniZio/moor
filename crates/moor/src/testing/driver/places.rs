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

#[cfg(test)]
mod tests;
