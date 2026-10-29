//! The fake's acquire: the Box it hands out and the notes it reports first.

use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxHandle;

use super::script::layout;
use super::FakeDriver;

impl FakeDriver {
    /// A new Box for `req`, or the failure set up for it.
    pub(super) fn acquired(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        let mut s = self.st();
        s.note("acquire".to_owned());
        if let Some(e) = s.fail_acquire.take() {
            return Err(e);
        }
        s.next += 1;
        let id = format!("box-{}", s.next);
        s.live.push(id.clone());
        s.providers.push((id.clone(), req.provider.clone()));
        s.cache.plans.push(req.cache.clone());
        s.cache.nonces.push(req.nonce.clone());
        let workdir = layout(s.work_root.as_deref(), &id)?;
        Ok(Acquired {
            handle: BoxHandle::new(id),
            workdir,
        })
    }

    /// Hands `note` the notes set up for an acquire, then runs the hook set up for during it.
    pub(super) fn replay_notes(&self, note: &mut dyn FnMut(&str)) {
        let (notes, during) = {
            let s = self.st();
            (s.acquire_notes.clone(), s.during_acquire.clone())
        };
        for n in &notes {
            note(n);
        }
        if let Some(during) = during {
            during();
        }
    }
}
