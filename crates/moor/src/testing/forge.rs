//! Scripted `Forge` for unit tests.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo, GitAuthor, RepoAt, RepoFiles};
use moor_core::{Secret, Sha};

#[derive(Default)]
struct State {
    repos: HashMap<String, ForgeRepo>,
    reads: HashMap<ForgeRepo, Result<RepoAt, ForgeError>>,
    files: HashMap<(ForgeRepo, Option<String>, &'static str), FileAnswer>,
    asked: Vec<Read>,
    files_asked: Vec<(ForgeRepo, Option<String>, &'static str)>,
}

/// What `file_at` answers: the commit and the file's text there.
pub(crate) type FileAnswer = Result<(Sha, Option<String>), ForgeError>;

/// One `read` the fake answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Read {
    /// The repo read.
    pub(crate) repo: ForgeRepo,
    /// The base asked for.
    pub(crate) base: Option<String>,
    /// The branch asked for.
    pub(crate) branch: String,
    /// The token's bytes, as text.
    pub(crate) token: String,
}

/// What a forge answers for a repo at `base` whose `.moor.yaml` is `moor_yaml`, whose Box
/// branch is new and that the token's account may push to.
pub(crate) fn repo_at(base: &str, moor_yaml: Option<&str>) -> RepoAt {
    RepoAt {
        base: Sha::parse(base).expect("hex sha"),
        branch_tip: None,
        files: RepoFiles {
            moor_yaml: moor_yaml.map(str::to_owned),
            ..RepoFiles::default()
        },
        author: GitAuthor {
            name: "Ann Dev".to_owned(),
            email: "1+ann@users.noreply.github.com".to_owned(),
        },
        can_push: true,
    }
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
    pub(crate) fn with_read(self, repo: ForgeRepo, answer: Result<RepoAt, ForgeError>) -> Self {
        self.answer(repo, answer);
        self
    }

    /// From now on answers every `read` of `repo` with `answer`.
    pub(crate) fn answer(&self, repo: ForgeRepo, answer: Result<RepoAt, ForgeError>) {
        self.lock().reads.insert(repo, answer);
    }

    /// Answers `file_at` of `path` in `repo` at `at` with `answer`.
    pub(crate) fn with_file(
        self,
        repo: ForgeRepo,
        at: Option<&str>,
        path: &'static str,
        answer: FileAnswer,
    ) -> Self {
        self.lock()
            .files
            .insert((repo, at.map(str::to_owned), path), answer);
        self
    }

    /// Every `file_at` asked so far, as repo, ref and path, in order.
    pub(crate) fn files_asked(&self) -> Vec<(ForgeRepo, Option<String>, &'static str)> {
        self.lock().files_asked.clone()
    }

    /// Every `read` answered so far, in order.
    pub(crate) fn reads(&self) -> Vec<Read> {
        self.lock().asked.clone()
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
        base: Option<&str>,
        branch: &str,
        token: &Secret,
    ) -> Result<RepoAt, ForgeError> {
        let mut state = self.lock();
        state.asked.push(Read {
            repo: repo.clone(),
            base: base.map(str::to_owned),
            branch: branch.to_owned(),
            token: String::from_utf8_lossy(token.expose()).into_owned(),
        });
        state
            .reads
            .get(repo)
            .cloned()
            .unwrap_or(Err(ForgeError::NotVisible))
    }

    fn file_at(
        &self,
        repo: &ForgeRepo,
        at: Option<&str>,
        path: &'static str,
        _token: &Secret,
    ) -> FileAnswer {
        let key = (repo.clone(), at.map(str::to_owned), path);
        let mut state = self.lock();
        state.files_asked.push(key.clone());
        state
            .files
            .get(&key)
            .cloned()
            .unwrap_or(Err(ForgeError::NotVisible))
    }
}
