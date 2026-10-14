//! Scripted `ProxyHost` for unit tests.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::seam::proxy_host::{
    ProxyEndpoint, ProxyHost, ProxyHostError, ProxySpec, ProxyStatus,
};
use moor_core::{Secret, TaskId};
use zeroize::Zeroizing;

/// Box port returned by an unscripted `ensure`.
pub(crate) const FAKE_BOX_PORT: u16 = 40123;

/// Nonce bytes returned by an unscripted `ensure`.
pub(crate) const FAKE_NONCE: &[u8] = b"fake-proxy-nonce";

/// A recorded `ProxyHost` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProxyHostCall {
    Ensure(ProxySpec),
    Status(TaskId),
    Stop(TaskId),
}

struct State {
    box_port: u16,
    nonce: Zeroizing<Vec<u8>>,
    ensure_error: Option<ProxyHostError>,
    status: Result<ProxyStatus, ProxyHostError>,
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
                nonce: Zeroizing::new(FAKE_NONCE.to_vec()),
                ensure_error: None,
                status: Ok(ProxyStatus::Absent),
                stop_error: None,
                calls: Vec::new(),
            })),
        }
    }

    fn with_state(self, f: impl FnOnce(&mut State)) -> Self {
        f(&mut self.lock());
        self
    }

    /// Makes `ensure` return `box_port` and a fresh secret holding `nonce`.
    pub(crate) fn with_endpoint(self, box_port: u16, nonce: &[u8]) -> Self {
        self.with_state(|s| {
            s.box_port = box_port;
            s.nonce = Zeroizing::new(nonce.to_vec());
        })
    }

    /// Makes `ensure` fail with `error`.
    pub(crate) fn with_ensure_error(self, error: ProxyHostError) -> Self {
        self.with_state(|s| s.ensure_error = Some(error))
    }

    /// Makes `status` return `status`.
    pub(crate) fn with_status(self, status: ProxyStatus) -> Self {
        self.with_state(|s| s.status = Ok(status))
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

/// Debug output never includes nonce bytes.
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
            nonce: Secret::new(s.nonce.to_vec()),
        })
    }

    fn status(&self, task: TaskId) -> Result<ProxyStatus, ProxyHostError> {
        let mut s = self.lock();
        s.calls.push(ProxyHostCall::Status(task));
        s.status.clone()
    }

    fn stop(&self, task: TaskId) -> Result<(), ProxyHostError> {
        let mut s = self.lock();
        s.calls.push(ProxyHostCall::Stop(task));
        s.stop_error.clone().map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests;
