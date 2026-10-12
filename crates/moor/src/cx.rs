//! Per-invocation context handed to operations.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::{RepoConfig, UserConfig};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::rng::Rng;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, Warning};

use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::config_load::load_repo;
use crate::error::CommandError;
use crate::store::Store;

/// File name of the state store inside the state directory.
const STORE_FILE: &str = "moor.db";

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
    /// Git adapter.
    git: Git,
    /// User configuration.
    config: UserConfig,
    /// Credential reader.
    creds: Creds,
    /// Invocation directory.
    cwd: PathBuf,
    /// State store, opened on first use.
    store: OnceCell<Store>,
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
    /// Git adapter.
    pub git: Git,
    /// User configuration.
    pub config: UserConfig,
    /// Credential reader.
    pub creds: Creds,
    /// Invocation directory.
    pub cwd: PathBuf,
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
            git: p.git,
            config: p.config,
            creds: p.creds,
            cwd: p.cwd,
            store: OnceCell::new(),
        }
    }

    /// The state store at `<state_dir>/moor.db`, opened on first call.
    pub(crate) fn store(&self) -> Result<&Store, CommandError> {
        if let Some(s) = self.store.get() {
            return Ok(s);
        }
        let dir = self.state_dir.as_deref().ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                "no state directory: set MOOR_STATE_DIR, XDG_STATE_HOME or HOME",
            )
        })?;
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

    /// Git adapter.
    pub(crate) const fn git(&self) -> &Git {
        &self.git
    }

    /// User configuration.
    pub(crate) const fn config(&self) -> &UserConfig {
        &self.config
    }

    /// Credential reader.
    pub(crate) const fn creds(&self) -> &Creds {
        &self.creds
    }

    /// Repo config for `worktree`: its own `.moor.yaml`, else the main checkout's.
    pub(crate) fn repo_config(&self, worktree: &Path) -> Result<Option<RepoConfig>, CommandError> {
        let main = self
            .git
            .main_checkout(worktree)
            .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
        load_repo(worktree, &main)
    }

    /// Invocation directory.
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Time source for this run.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by tests"))]
    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Randomness source for this run.
    pub(crate) fn rng(&self) -> &dyn Rng {
        self.rng.as_ref()
    }

    /// Record a warning for the final response.
    pub(crate) fn warn(&self, w: Warning) {
        self.warnings.borrow_mut().push(w);
    }

    /// Drain recorded warnings.
    pub(crate) fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }
}

#[cfg(test)]
mod tests;
