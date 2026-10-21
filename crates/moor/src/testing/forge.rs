//! Scripted `Forge` for unit tests.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use moor_core::seam::forge::{Forge, ForgeRepo};

#[derive(Default)]
struct State {
    repos: HashMap<String, ForgeRepo>,
}

/// Scripted forge; unscripted origins have no repo.
#[derive(Default)]
pub(crate) struct FakeForge {
    state: Mutex<State>,
}

impl FakeForge {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn with_state(mut self, f: impl FnOnce(&mut State)) -> Self {
        f(self.state.get_mut().unwrap_or_else(PoisonError::into_inner));
        self
    }

    /// Maps `origin` to `repo` for `repo_of`.
    pub(crate) fn with_repo(self, origin: &str, repo: ForgeRepo) -> Self {
        self.with_state(|s| {
            s.repos.insert(origin.to_owned(), repo);
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for FakeForge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeForge")
            .field("repos", &self.lock().repos)
            .finish()
    }
}

impl Forge for FakeForge {
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        self.lock().repos.get(origin).cloned()
    }
}
