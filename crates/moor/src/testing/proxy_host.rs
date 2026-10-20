//! Scripted `ProxyHost` for unit tests.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::seam::proxy_host::{
    ProxyEndpoint, ProxyHost, ProxyHostError, ProxySpec, ProxyStatus,
};
use moor_core::BoxId;

/// Box port returned by an unscripted `ensure`.
pub(crate) const FAKE_BOX_PORT: u16 = 40123;

/// A recorded `ProxyHost` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProxyHostCall {
    Ensure(ProxySpec),
    Status(BoxId),
    Stop(BoxId),
}

struct State {
    box_port: u16,
    ensure_error: Option<ProxyHostError>,
    status: Result<ProxyStatus, ProxyHostError>,
    upcoming: VecDeque<ProxyStatus>,
    stop_error: Option<ProxyHostError>,
    calls: Vec<ProxyHostCall>,
}

/// Scripted proxy host; clones share one script and one call log.
#[derive(Clone)]
pub(crate) struct FakeProxyHost {
    state: Arc<Mutex<State>>,
}

impl FakeProxyHost {
    /// Succeeds everywhere: the default endpoint, status `Absent`, stop `Ok`.
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                box_port: FAKE_BOX_PORT,
                ensure_error: None,
                status: Ok(ProxyStatus::Absent),
                upcoming: VecDeque::new(),
                stop_error: None,
                calls: Vec::new(),
            })),
        }
    }

    fn with_state(self, f: impl FnOnce(&mut State)) -> Self {
        f(&mut self.lock());
        self
    }

    /// Makes `ensure` fail with `error`.
    pub(crate) fn with_ensure_error(self, error: ProxyHostError) -> Self {
        self.with_state(|s| s.ensure_error = Some(error))
    }

    /// Makes `status` return `status`.
    pub(crate) fn with_status(self, status: ProxyStatus) -> Self {
        self.with_state(|s| s.status = Ok(status))
    }

    /// Makes the next `status` calls return `statuses` in order, then the usual status.
    pub(crate) fn with_status_script(
        self,
        statuses: impl IntoIterator<Item = ProxyStatus>,
    ) -> Self {
        self.with_state(|s| s.upcoming.extend(statuses))
    }

    /// Makes `stop` fail with `error`.
    pub(crate) fn with_stop_error(self, error: ProxyHostError) -> Self {
        self.with_state(|s| s.stop_error = Some(error))
    }

    /// Returns the calls made so far, in order.
    pub(crate) fn calls(&self) -> Vec<ProxyHostCall> {
        self.lock().calls.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for FakeProxyHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.lock();
        f.debug_struct("FakeProxyHost")
            .field("box_port", &s.box_port)
            .field("calls", &s.calls)
            .finish_non_exhaustive()
    }
}

impl ProxyHost for FakeProxyHost {
    fn ensure(&self, spec: &ProxySpec) -> Result<ProxyEndpoint, ProxyHostError> {
        let mut s = self.lock();
        s.calls.push(ProxyHostCall::Ensure(spec.clone()));
        if let Some(e) = &s.ensure_error {
            return Err(e.clone());
        }
        Ok(ProxyEndpoint {
            box_port: s.box_port,
            ca_pem: String::new(),
        })
    }

    fn status(&self, box_id: BoxId) -> Result<ProxyStatus, ProxyHostError> {
        let mut s = self.lock();
        s.calls.push(ProxyHostCall::Status(box_id));
        if let Some(next) = s.upcoming.pop_front() {
            return Ok(next);
        }
        s.status.clone()
    }

    fn stop(&self, box_id: BoxId) -> Result<(), ProxyHostError> {
        let mut s = self.lock();
        s.calls.push(ProxyHostCall::Stop(box_id));
        s.stop_error.clone().map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests;
