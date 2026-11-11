//! What a context shares with the others in its controller: the owners of every Box, the nodes
//! promoting, and the detached context built from them.

use std::cell::{OnceCell, RefCell};
use std::sync::Arc;

use moor_core::request::Pins;
use moor_core::user::Viewer;

use super::in_flight::InFlight;
use super::owners::Owners;
use super::Cx;
use crate::env::Cancel;
use crate::error::CommandError;

impl Cx {
    /// [`Cx::owners`], to share.
    pub(crate) fn owners_arc(&self) -> &Arc<Owners> {
        self.owners.get_or_init(|| {
            self.env.shared::<Arc<Owners>>().map_or_else(
                || Arc::new(Owners::new(Arc::clone(&self.clock))),
                Arc::clone,
            )
        })
    }

    /// This context sharing `owners` with the others that use it.
    #[cfg(test)]
    pub(crate) fn with_owners(self, owners: Arc<Owners>) -> Self {
        Self {
            owners: OnceCell::from(owners),
            ..self
        }
    }

    /// The nodes with a promotion running: one set per controller, shared through the env, else
    /// this context's own.
    pub(crate) fn promoting(&self) -> &Arc<InFlight> {
        self.promoting.get_or_init(|| {
            self.env
                .shared::<Arc<InFlight>>()
                .map_or_else(|| Arc::new(InFlight::default()), Arc::clone)
        })
    }

    /// A context for work on a thread of its own that outlives this request: the same adapters,
    /// store, owners and promoting nodes, its own warnings, acting as the operator, and stopped
    /// by no request.
    ///
    /// # Errors
    /// The store cannot be opened.
    pub(crate) fn detached(&self) -> Result<Self, CommandError> {
        let store = self.store()?.clone();
        Ok(Self {
            state_dir: self.state_dir.clone(),
            warnings: RefCell::new(Vec::new()),
            clock: Arc::clone(&self.clock),
            rng: Arc::clone(&self.rng),
            driver: Arc::clone(&self.driver),
            engine: Arc::clone(&self.engine),
            proc: Arc::clone(&self.proc),
            forge: Arc::clone(&self.forge),
            proxy_host: Arc::clone(&self.proxy_host),
            machine_links: self.machine_links.clone(),
            config: self.config.clone(),
            env: self.env.clone().with_cancel(Cancel::default()),
            prompt_window: self.prompt_window,
            store: OnceCell::from(store),
            owners: OnceCell::from(Arc::clone(self.owners_arc())),
            promoting: OnceCell::from(Arc::clone(self.promoting())),
            blobs: self
                .blobs
                .get()
                .cloned()
                .map_or_else(OnceCell::new, OnceCell::from),
            vault_key: self.vault_key.clone(),
            requester: None,
            viewer: Viewer::Operator,
            pins: Pins::new(),
            #[cfg(test)]
            claimed: self.claimed,
        })
    }
}
