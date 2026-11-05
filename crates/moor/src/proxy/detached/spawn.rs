//! Starting a supervisor process, and the mode it records so a host never reuses one started
//! for another mode.

use std::net::SocketAddr;
use std::path::Path;

use moor_core::seam::proc::{Deadline, ProcSpec, Tool};
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::BoxId;

use super::key::mint_key;
use super::{fail, io_failed, store_failed, DetachedSupervisor, WAIT};
use crate::env::{EGRESS_RELAY, PROXY_KEY};
use crate::proxy::ledger::LEDGER;
use crate::store::Store;

/// The bit a supervisor that relays its tunnel to the shared egress Proxy adds to the ledger
/// protocol its row records.
const RELAYING: u32 = 1 << 16;

impl DetachedSupervisor {
    /// What a supervisor this host starts records in its row's `ledger`: the ledger protocol it
    /// speaks, plus [`RELAYING`] when it relays its tunnel. A row recorded otherwise is replaced,
    /// never reused, so turning the shared Proxy on or off restarts every supervisor.
    pub(crate) const fn started_as(&self) -> u32 {
        if self.relay.is_some() {
            LEDGER | RELAYING
        } else {
            LEDGER
        }
    }

    /// This host starting supervisors that relay their tunnel to the shared egress Proxy at
    /// `relay`, when there is one.
    #[must_use]
    pub(crate) const fn relaying(mut self, relay: Option<SocketAddr>) -> Self {
        self.relay = relay;
        self
    }

    /// Starts a supervisor for `box_id` with a fresh key; returns its pid.
    pub(super) fn spawn(
        &self,
        store: &Store,
        (state, log): (&Path, &Path),
        box_id: BoxId,
    ) -> Result<u32, ProxyHostError> {
        let key = mint_key().map_err(|e| io_failed(&e, log))?;
        store
            .set_proxy_key(box_id, key.as_bytes(), self.started_as())
            .map_err(|e| store_failed(&e, log))?;
        let mut spec = ProcSpec::new(Tool::MoorSelf, Deadline(WAIT))
            .args(["proxy", "serve", &box_id.get().to_string()])
            // The controller may run on a state dir its own config chose.
            .env("MOOR_STATE_DIR", state.to_string_lossy())
            .env(PROXY_KEY, key.as_str());
        if let Some(relay) = self.relay {
            spec = spec.env(EGRESS_RELAY, relay.to_string());
        }
        self.proc
            .spawn_daemon(&spec, log)
            .map_err(|e| fail(&format!("the proxy supervisor could not start: {e}"), log))
    }
}
