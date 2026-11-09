//! A turn an op waits for only so long.

use std::time::Duration;

use moor_core::BoxId;

use super::owners::{Cmd, Turn};
use super::Cx;
use crate::error::CommandError;

impl Cx {
    /// Box `id`'s turn for `cmd`, waiting up to `wait` behind the ops before it; unlike
    /// [`Cx::try_turn`], a run in flight does not keep it out.
    ///
    /// # Errors
    /// As [`super::owners::Owners::turn`].
    pub(crate) fn turn_within(
        &self,
        id: BoxId,
        cmd: Cmd,
        wait: Duration,
    ) -> Result<Turn<'_>, CommandError> {
        let deadline = self.clock.instant() + wait;
        self.owners().turn(id, cmd, self.env.cancel(), deadline)
    }
}
