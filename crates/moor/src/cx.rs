//! Per-invocation context handed to operations.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::request::Pins;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::Engine;
use moor_core::seam::forge::Forge;
use moor_core::seam::proc::Proc;
use moor_core::seam::proxy_host::ProxyHost;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::user::{Identity, UserId, Viewer};
use moor_core::{ErrorKind, Warning};

use crate::env::Env;
use crate::error::CommandError;
use crate::store::{Store, STORE_FILE};

/// State shared by one command run.
pub(crate) struct Cx {
    /// Resolved state directory.
    state_dir: Option<PathBuf>,
    /// Warnings recorded so far.
    warnings: RefCell<Vec<Warning>>,
    /// Time source; the only way ops read time.
    clock: Arc<dyn Clock>,
    /// Randomness source.
    rng: Arc<dyn Rng>,
    /// Box backend.
    driver: Arc<dyn Driver>,
    /// Workspace backend.
    surface: Arc<dyn Surface>,
    /// The agent running in a Box.
    engine: Arc<dyn Engine>,
    /// Process runner.
    proc: Arc<dyn Proc>,
    /// Code forge.
    forge: Arc<dyn Forge>,
    /// Credential proxy host.
    proxy_host: Arc<dyn ProxyHost>,
    /// User configuration.
    config: UserConfig,
    /// Host environment snapshot.
    env: Env,
    /// How long a fresh agent pane is watched for first-run prompts.
    prompt_window: Duration,
    /// State store, opened on first use.
    store: OnceCell<Store>,
    /// The controller's vault key file; `None` outside a controller.
    vault_key: Option<PathBuf>,
    /// The identity a socket request acts as, already checked against its socket.
    requester: Option<Identity>,
    /// Whose Boxes this request may see; the operator outside a controller socket.
    viewer: Viewer,
    /// The Box each name meant when the controller let this request name it.
    pins: Pins,
}

/// Everything a context is built from.
pub(crate) struct CxParts {
    /// Resolved state directory.
    pub state_dir: Option<PathBuf>,
    /// Time source.
    pub clock: Arc<dyn Clock>,
    /// Randomness source.
    pub rng: Arc<dyn Rng>,
    /// Box backend.
    pub driver: Arc<dyn Driver>,
    /// Workspace backend.
    pub surface: Arc<dyn Surface>,
    /// The agent running in a Box.
    pub engine: Arc<dyn Engine>,
    /// Process runner.
    pub proc: Arc<dyn Proc>,
    /// Code forge.
    pub forge: Arc<dyn Forge>,
    /// Credential proxy host.
    pub proxy_host: Arc<dyn ProxyHost>,
    /// User configuration.
    pub config: UserConfig,
    /// Host environment snapshot.
    pub env: Env,
    /// How long a fresh agent pane is watched for first-run prompts.
    pub prompt_window: Duration,
}

impl Cx {
    /// Create a context; the store stays closed until first use.
    pub(crate) fn new(p: CxParts) -> Self {
        Self {
            state_dir: p.state_dir,
            warnings: RefCell::new(Vec::new()),
            clock: p.clock,
            rng: p.rng,
            driver: p.driver,
            surface: p.surface,
            engine: p.engine,
            proc: p.proc,
            forge: p.forge,
            proxy_host: p.proxy_host,
            config: p.config,
            env: p.env,
            prompt_window: p.prompt_window,
            store: OnceCell::new(),
            vault_key: None,
            requester: None,
            viewer: Viewer::Operator,
            pins: Pins::new(),
        }
    }

    /// The resolved state directory.
    pub(crate) fn state_dir(&self) -> Result<&Path, CommandError> {
        self.state_dir.as_deref().ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                "no state directory: set MOOR_STATE_DIR, XDG_STATE_HOME or HOME",
            )
        })
    }

    /// The store the env shares, else the one at `<state_dir>/moor.db`; opened on first call.
    pub(crate) fn store(&self) -> Result<&Store, CommandError> {
        if let Some(s) = self.store.get() {
            return Ok(s);
        }
        let store = match self.env.shared::<Store>() {
            Some(shared) => shared.clone(),
            None => Store::open(&self.state_dir()?.join(STORE_FILE), Arc::clone(&self.clock))?,
        };
        Ok(self.store.get_or_init(|| store.pinned(self.pins.clone())))
    }

    /// Box backend.
    pub(crate) fn driver(&self) -> &dyn Driver {
        self.driver.as_ref()
    }

    /// Workspace backend.
    pub(crate) fn surface(&self) -> &dyn Surface {
        self.surface.as_ref()
    }

    /// The agent running in a Box.
    pub(crate) fn engine(&self) -> &dyn Engine {
        self.engine.as_ref()
    }

    /// How long a fresh agent pane is watched for first-run prompts.
    pub(crate) const fn prompt_window(&self) -> Duration {
        self.prompt_window
    }

    /// This context answering a request that the returned flag stops.
    #[cfg(test)]
    pub(crate) fn cancellable(mut self) -> (Self, crate::env::Cancel) {
        let cancel = crate::env::Cancel::default();
        let env = std::mem::take(&mut self.env).with_cancel(cancel.clone());
        (Self { env, ..self }, cancel)
    }

    /// This context watching for first-run prompts for `window`.
    #[cfg(test)]
    pub(crate) const fn with_prompt_window(mut self, window: Duration) -> Self {
        self.prompt_window = window;
        self
    }

    /// This context with `engine` in place of the current one.
    #[cfg(test)]
    pub(crate) fn with_engine(mut self, engine: Arc<dyn Engine>) -> Self {
        self.engine = engine;
        self
    }

    /// Process runner.
    pub(crate) fn proc(&self) -> &dyn Proc {
        self.proc.as_ref()
    }

    /// Code forge.
    pub(crate) fn forge(&self) -> &dyn Forge {
        self.forge.as_ref()
    }

    /// This context with `forge` in place of the current one.
    #[cfg(test)]
    pub(crate) fn with_forge(mut self, forge: Arc<dyn Forge>) -> Self {
        self.forge = forge;
        self
    }

    /// Credential proxy host.
    pub(crate) fn proxy_host(&self) -> &dyn ProxyHost {
        self.proxy_host.as_ref()
    }

    /// This context with `host` in place of the current proxy host.
    #[cfg(test)]
    pub(crate) fn with_proxy_host(mut self, host: Arc<dyn ProxyHost>) -> Self {
        self.proxy_host = host;
        self
    }

    /// User configuration.
    pub(crate) const fn config(&self) -> &UserConfig {
        &self.config
    }

    /// Host environment snapshot.
    pub(crate) const fn env(&self) -> &Env {
        &self.env
    }

    /// Time source for this run.
    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// This context with `config` in place of the current one.
    #[cfg(test)]
    pub(crate) fn with_config(mut self, config: UserConfig) -> Self {
        self.config = config;
        self
    }

    /// This context with `clock` in place of the current one.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Randomness source for this run.
    pub(crate) fn rng(&self) -> &dyn Rng {
        self.rng.as_ref()
    }

    /// Record a warning for the final response.
    pub(crate) fn warn(&self, w: Warning) {
        self.warnings.borrow_mut().push(w);
    }

    /// Runs `f` for a Box that acts for `owner`, keeping the warnings it records only when the
    /// caller may see that Box, so a reply never names a Box hidden from its caller.
    pub(crate) fn for_box<T>(&self, owner: Option<UserId>, f: impl FnOnce() -> T) -> T {
        if self.viewer.may_act_on(owner) {
            return f();
        }
        let before = self.warnings.borrow().len();
        let out = f();
        self.warnings.borrow_mut().truncate(before);
        out
    }

    /// Drain recorded warnings.
    pub(crate) fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }
}

mod requester;
#[cfg(test)]
mod tests;
