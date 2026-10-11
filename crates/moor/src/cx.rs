//! Per-invocation context handed to operations.

use std::cell::{OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::UserConfig;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::seam::surface::Surface;
use moor_core::{ErrorKind, Warning};

use crate::adapters::git::Git;
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
    /// Box backend.
    driver: Arc<dyn Driver>,
    /// Workspace backend.
    surface: Arc<dyn Surface>,
    /// Git adapter.
    git: Git,
    /// User configuration.
    config: UserConfig,
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
    /// Box backend.
    pub driver: Arc<dyn Driver>,
    /// Workspace backend.
    pub surface: Arc<dyn Surface>,
    /// Git adapter.
    pub git: Git,
    /// User configuration.
    pub config: UserConfig,
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
            driver: p.driver,
            surface: p.surface,
            git: p.git,
            config: p.config,
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

    /// Invocation directory.
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Time source for this run.
    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Record a warning for the final response.
    pub(crate) fn warn(&self, w: Warning) {
        self.warnings.borrow_mut().push(w);
    }

    /// Drain recorded warnings.
    pub(crate) fn take_warnings(&self) -> Vec<Warning> {
        std::mem::take(&mut *self.warnings.borrow_mut())
    }

    /// Test context: fake clock, default config, git over `proc`.
    #[cfg(test)]
    pub(crate) fn fake(
        state_dir: &Path,
        cwd: &Path,
        driver: Arc<dyn Driver>,
        surface: Arc<dyn Surface>,
        proc: Arc<dyn moor_core::seam::proc::Proc>,
    ) -> Self {
        use crate::adapters::clock::FakeClock;
        use moor_core::seam::clock::Ts;
        Self::new(CxParts {
            state_dir: Some(state_dir.to_path_buf()),
            clock: Arc::new(FakeClock::new(Ts::from_millis(1000))),
            driver,
            surface,
            git: Git::new(proc),
            config: UserConfig::default(),
            cwd: cwd.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests;
