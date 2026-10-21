//! Per-invocation context handed to operations.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::{RepoConfig, RepoTable, UserConfig};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::Engine;
use moor_core::seam::forge::Forge;
use moor_core::seam::proc::Proc;
use moor_core::seam::proxy_host::ProxyHost;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::user::{Identity, Viewer};
use moor_core::{ErrorKind, Warning};

use crate::adapters::git::Git;
use crate::config_load::load_repo;
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
    /// Git adapter.
    git: Git,
    /// User configuration.
    config: UserConfig,
    /// Host environment snapshot.
    env: Env,
    /// Invocation directory.
    cwd: PathBuf,
    /// How long a fresh agent pane is watched for first-run prompts.
    prompt_window: Duration,
    /// State store, opened on first use.
    store: OnceCell<Store>,
    /// Repos the controller knows.
    repos: Arc<RepoTable>,
    /// The controller's vault key file; `None` outside a controller.
    vault_key: Option<PathBuf>,
    /// The identity a socket request acts as, already checked against its socket.
    requester: Option<Identity>,
    /// Whose Boxes this request may see; the operator outside a controller socket.
    viewer: Viewer,
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
    /// Git adapter.
    pub git: Git,
    /// User configuration.
    pub config: UserConfig,
    /// Host environment snapshot.
    pub env: Env,
    /// Invocation directory.
    pub cwd: PathBuf,
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
            git: p.git,
            config: p.config,
            env: p.env,
            cwd: p.cwd,
            prompt_window: p.prompt_window,
            store: OnceCell::new(),
            repos: Arc::new(RepoTable::new()),
            vault_key: None,
            requester: None,
            viewer: Viewer::Operator,
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

    /// The state store at `<state_dir>/moor.db`, opened on first call.
    pub(crate) fn store(&self) -> Result<&Store, CommandError> {
        if let Some(s) = self.store.get() {
            return Ok(s);
        }
        let dir = self.state_dir()?;
        let store = Store::open(&dir.join(STORE_FILE), Arc::clone(&self.clock))?;
        Ok(self.store.get_or_init(|| store))
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

    /// Git adapter.
    pub(crate) const fn git(&self) -> &Git {
        &self.git
    }

    /// User configuration.
    pub(crate) const fn config(&self) -> &UserConfig {
        &self.config
    }

    /// Host environment snapshot.
    pub(crate) const fn env(&self) -> &Env {
        &self.env
    }

    /// Repo config for `worktree`: its own `.moor.yaml`, else the main checkout's.
    pub(crate) fn repo_config(&self, worktree: &Path) -> Result<Option<RepoConfig>, CommandError> {
        let main = self
            .git
            .main_checkout(worktree)
            .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
        load_repo(worktree, &main)?.map_or_else(|| self.overlay_config(&main), |c| Ok(Some(c)))
    }

    /// Repo config of the main checkout `worktree` belongs to, ignoring the worktree's own file.
    pub(crate) fn main_repo_config(
        &self,
        worktree: &Path,
    ) -> Result<Option<RepoConfig>, CommandError> {
        let main = self
            .git
            .main_checkout(worktree)
            .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
        load_repo(&main, &main)?.map_or_else(|| self.overlay_config(&main), |c| Ok(Some(c)))
    }

    /// Invocation directory.
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Time source for this run.
    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
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

    /// Records `w` unless a warning with the same code is already recorded.
    pub(crate) fn warn_once(&self, w: Warning) {
        let mut warnings = self.warnings.borrow_mut();
        if !warnings.iter().any(|x| x.code == w.code) {
            warnings.push(w);
        }
    }

    /// Drain recorded warnings.
    pub(crate) fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }
}

mod repos;
mod requester;
#[cfg(test)]
mod tests;
