//! Retrying crabbox calls that lose a claim race.

use std::time::Duration;

use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::ProcError;

use super::errors::{is_claim_race, is_tunnel_claim_race};
use super::Crabbox;

/// Total time a call waits out claim races; a restarting tunnel rewrites the claim every few seconds.
const CLAIM_RACE_BUDGET: Duration = Duration::from_secs(15);
/// Pause between retries of a claim race.
const CLAIM_RACE_PAUSE: Duration = Duration::from_secs(1);

/// A failure that may be crabbox losing a claim race.
pub(super) trait ClaimRace {
    /// Whether another call rewrote the lease claim.
    fn lost_claim_race(&self) -> bool;
}

impl ClaimRace for ProcError {
    fn lost_claim_race(&self) -> bool {
        is_claim_race(self)
    }
}

impl ClaimRace for DriverError {
    fn lost_claim_race(&self) -> bool {
        is_tunnel_claim_race(self)
    }
}

impl Crabbox {
    /// Runs `call`, trying again while crabbox reports a claim race, for up to `CLAIM_RACE_BUDGET` of waiting.
    pub(super) fn retry_claim_race<T, E: ClaimRace>(
        &self,
        mut call: impl FnMut() -> Result<T, E>,
    ) -> Result<T, E> {
        let mut waited = Duration::ZERO;
        loop {
            match call() {
                Err(e) if waited + CLAIM_RACE_PAUSE <= CLAIM_RACE_BUDGET && e.lost_claim_race() => {
                    self.clock.sleep(CLAIM_RACE_PAUSE);
                    waited += CLAIM_RACE_PAUSE;
                }
                result => return result,
            }
        }
    }
}
