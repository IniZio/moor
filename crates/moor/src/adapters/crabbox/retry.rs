//! Retrying crabbox calls that lose a claim race.

use std::time::Duration;

use moor_core::seam::proc::ProcError;

use super::errors::is_claim_race;
use super::Crabbox;

/// Total time a call waits out claim races; a restarting tunnel rewrites the claim every few seconds.
const CLAIM_RACE_BUDGET: Duration = Duration::from_secs(15);
/// Pause between retries of a claim race.
const CLAIM_RACE_PAUSE: Duration = Duration::from_secs(1);

impl Crabbox {
    /// Runs `call`, trying again while crabbox reports a claim race, for up to `CLAIM_RACE_BUDGET` of waiting.
    pub(super) fn retry_claim_race<T>(
        &self,
        mut call: impl FnMut() -> Result<T, ProcError>,
    ) -> Result<T, ProcError> {
        let mut waited = Duration::ZERO;
        loop {
            match call() {
                Err(e) if waited + CLAIM_RACE_PAUSE <= CLAIM_RACE_BUDGET && is_claim_race(&e) => {
                    self.clock.sleep(CLAIM_RACE_PAUSE);
                    waited += CLAIM_RACE_PAUSE;
                }
                result => return result,
            }
        }
    }
}
