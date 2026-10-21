//! Scripted `Forge` for unit tests.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, RepoAt};
use moor_core::Secret;

#[derive(Default)]
struct State {
    repos: HashMap<String, ForgeRepo>,
    reads: HashMap<ForgeRepo, Result<RepoAt, ForgeError>>,
}

/// Scripted forge; unscripted origins have no repo and unscripted repos are not visible.
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

    /// Answers every `read` of `repo` with `answer`.
    #[expect(dead_code, reason = "start reads the forge in a later change")]
    pub(crate) fn with_read(self, repo: ForgeRepo, answer: Result<RepoAt, ForgeError>) -> Self {
        self.with_state(|s| {
            s.reads.insert(repo, answer);
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
            .finish_non_exhaustive()
    }
}

impl Forge for FakeForge {
    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        self.lock().repos.get(origin).cloned()
    }

    fn read(
        &self,
        repo: &ForgeRepo,
        _base: Option<&str>,
        _branch: &str,
        _token: &Secret,
    ) -> Result<RepoAt, ForgeError> {
        self.lock()
            .reads
            .get(repo)
            .cloned()
            .unwrap_or(Err(ForgeError::NotVisible))
    }
}
