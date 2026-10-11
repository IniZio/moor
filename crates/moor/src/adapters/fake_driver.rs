//! In-memory `Driver` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; consumers land with the ops slices"
)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, Driver, DriverCaps, DriverError, EgressApplied,
    EgressSupport, Released, SleepKind,
};
use moor_core::{BoxHandle, BoxPath, ProviderId};

#[derive(Default)]
struct State {
    next: u32,
    live: Vec<String>,
    providers: Vec<(String, ProviderId)>,
    calls: Vec<String>,
    fail_acquire: Option<DriverError>,
    fail_release: Option<DriverError>,
    fail_release_always: Option<DriverError>,
    fail_list: Option<DriverError>,
}

/// Scripted Driver: boxes `box-1`, `box-2`, ... with workdir `/work/box-N`.
#[derive(Default)]
pub(crate) struct FakeDriver {
    state: Mutex<State>,
}

impl FakeDriver {
    /// An empty Driver with no scripted failures.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next `acquire` fails once with `e`.
    pub(crate) fn fail_acquire(self, e: DriverError) -> Self {
        self.st().fail_acquire = Some(e);
        self
    }

    /// The next `release` fails once with `e`.
    pub(crate) fn fail_release(self, e: DriverError) -> Self {
        self.st().fail_release = Some(e);
        self
    }

    /// Every `release` fails with `e`.
    pub(crate) fn fail_release_always(self, e: DriverError) -> Self {
        self.st().fail_release_always = Some(e);
        self
    }

    /// The next `list` fails once with `e`.
    pub(crate) fn fail_list(self, e: DriverError) -> Self {
        self.st().fail_list = Some(e);
        self
    }

    /// Makes `handle` vanish, as if deleted behind moor's back.
    pub(crate) fn forget(&self, handle: &BoxHandle) {
        self.st().live.retain(|h| h != handle.as_str());
    }

    /// Calls so far, like `acquire` or `release box-1`.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
    }
}

impl Driver for FakeDriver {
    fn caps(&self, _provider: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.st().calls.push("caps".to_owned());
        Ok(DriverCaps {
            sleep: SleepKind::None,
            wake: false,
            egress: EgressSupport::None,
            reverse_tunnel: false,
            has_workdir: true,
        })
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        let mut s = self.st();
        s.calls.push("acquire".to_owned());
        if let Some(e) = s.fail_acquire.take() {
            return Err(e);
        }
        s.next += 1;
        let id = format!("box-{}", s.next);
        s.live.push(id.clone());
        s.providers.push((id.clone(), req.provider.clone()));
        let workdir = BoxPath::under("/work", &id).map_err(|e| DriverError::Failed {
            summary: e.to_string(),
        })?;
        Ok(Acquired {
            handle: BoxHandle::new(id),
            workdir,
            egress: EgressApplied::NotEnforced,
        })
    }

    fn planned_handle(&self, _req: &AcquireReq) -> BoxHandle {
        BoxHandle::new(format!("box-{}", self.st().next + 1))
    }

    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("release {}", handle.as_str()));
        if let Some(e) = s
            .fail_release_always
            .clone()
            .or_else(|| s.fail_release.take())
        {
            return Err(e);
        }
        let before = s.live.len();
        s.live.retain(|h| h != handle.as_str());
        Ok(if s.live.len() < before {
            Released::Deleted
        } else {
            Released::AlreadyGone
        })
    }

    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        let mut s = self.st();
        s.calls.push("list".to_owned());
        if let Some(e) = s.fail_list.take() {
            return Err(e);
        }
        Ok(s.providers
            .iter()
            .filter(|(h, p)| p == provider && s.live.contains(h))
            .map(|(h, _)| BoxListing {
                handle: BoxHandle::new(h.clone()),
                provider: Some(provider.clone()),
                asleep: false,
            })
            .collect())
    }

    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError> {
        let mut s = self.st();
        s.calls.push(format!("workdir {}", handle.as_str()));
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        BoxPath::under("/work", handle.as_str()).map_err(|e| DriverError::Failed {
            summary: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests;
