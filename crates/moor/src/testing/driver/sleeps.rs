//! The Driver fake's sleep and wake: both log the call, fail on an unknown Box, then run the hook.

use moor_core::seam::driver::{DriverError, PausePublish, SleepOutcome};
use moor_core::BoxHandle;

use super::FakeDriver;

impl FakeDriver {
    pub(super) fn local_sleep(
        &self,
        handle: &BoxHandle,
        publish: PausePublish,
    ) -> Result<SleepOutcome, DriverError> {
        let mut s = self.st();
        s.cache.pause_publishes.push(publish);
        s.note(format!("sleep {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_sleep.take() {
            return Err(e);
        }
        let (hook, outcome) = (s.on_sleep.clone(), s.sleep_outcome.clone());
        drop(s);
        if let Some(hook) = hook {
            hook();
        }
        Ok(outcome.unwrap_or(SleepOutcome::Slept))
    }

    pub(super) fn local_wake(&self, handle: &BoxHandle) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("wake {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if let Some(e) = s.fail_wake.take() {
            return Err(e);
        }
        let hook = s.on_wake.clone();
        drop(s);
        if let Some(hook) = hook {
            hook();
        }
        Ok(())
    }
}
