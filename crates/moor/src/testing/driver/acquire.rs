//! The fake's acquire: the Box it hands out and the notes it reports first.

use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxHandle;

use super::script::layout;
use super::FakeDriver;

/// The acquire that goes on when its caller is cut off, as a pool host's does.
#[derive(Default)]
pub(super) struct Flight {
    /// Whether a cut acquire goes on and keeps its slug.
    pub(super) goes_on: bool,
    /// The handle of the acquire going on.
    pub(super) at: Option<String>,
}

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
        s.cache.classes.push(req.class.clone());
        if let Some(trees) = &s.trees {
            trees.acquired(&id)?;
        }
        let workdir = layout(s.work_root.as_deref(), &id)?;
        Ok(Acquired {
            handle: BoxHandle::new(id),
            workdir,
        })
    }

    /// Starts an acquire; refused while one goes on under the same slug, as crabbox refuses it.
    pub(super) fn take_off(&self) -> Result<(), DriverError> {
        let mut s = self.st();
        if !s.flight.goes_on {
            return Ok(());
        }
        let planned = format!("box-{}", s.next + 1);
        if s.flight.at.as_deref() == Some(planned.as_str()) {
            return Err(DriverError::Failed {
                summary: format!(
                    "crabbox warmup failed (Some(4)): requested external lease slug \"{planned}\" \
                     is already in use in this lifecycle scope"
                ),
            });
        }
        s.flight.at = Some(planned);
        Ok(())
    }

    /// Takes over the acquire going on.
    pub(super) fn adopted(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.st().note("adopt".to_owned());
        self.land(req, note)
    }

    /// Ends the acquire going on, handing `note` its notes.
    pub(super) fn land(
        &self,
        req: &AcquireReq,
        note: &mut dyn FnMut(&str),
    ) -> Result<Acquired, DriverError> {
        self.replay_notes(note);
        let got = self.acquired(req);
        self.st().flight.at = None;
        got
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
