//! Scripted `Forge` for unit tests.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use moor_core::proxy_gate::RouteRow;
use moor_core::seam::forge::{Forge, ForgeError, ForgeRepo};
use moor_core::Secret;
use zeroize::Zeroizing;

/// A recorded `Forge` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgeCall {
    Routes,
    RepoOf(String),
    RepoToken(ForgeRepo),
}

/// A canned token outcome. Token bytes are wiped on drop.
enum TokenReply {
    Token(Zeroizing<Vec<u8>>),
    Err(ForgeError),
}

#[derive(Default)]
struct State {
    routes: Vec<RouteRow>,
    repos: HashMap<String, ForgeRepo>,
    tokens: HashMap<ForgeRepo, TokenReply>,
    calls: Vec<ForgeCall>,
}

/// Scripted forge; unscripted origins have no repo and unscripted repos fail.
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

    /// Sets the rows returned by `routes`.
    pub(crate) fn with_routes(self, routes: Vec<RouteRow>) -> Self {
        self.with_state(|s| s.routes = routes)
    }

    /// Maps `origin` to `repo` for `repo_of`.
    pub(crate) fn with_repo(self, origin: &str, repo: ForgeRepo) -> Self {
        self.with_state(|s| {
            s.repos.insert(origin.to_owned(), repo);
        })
    }

    /// Makes `repo_token` return a fresh secret holding `token` for `repo`.
    pub(crate) fn with_token(self, repo: ForgeRepo, token: &[u8]) -> Self {
        self.with_state(|s| {
            s.tokens
                .insert(repo, TokenReply::Token(Zeroizing::new(token.to_vec())));
        })
    }

    /// Makes `repo_token` fail with `error` for `repo`.
    pub(crate) fn with_token_error(self, repo: ForgeRepo, error: ForgeError) -> Self {
        self.with_state(|s| {
            s.tokens.insert(repo, TokenReply::Err(error));
        })
    }

    /// Returns the calls made so far, in order.
    pub(crate) fn calls(&self) -> Vec<ForgeCall> {
        self.lock().calls.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Debug output never includes token bytes.
impl fmt::Debug for FakeForge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.lock();
        f.debug_struct("FakeForge")
            .field("routes", &s.routes)
            .field("repos", &s.repos)
            .field("tokens", &s.tokens.keys().collect::<Vec<_>>())
            .field("calls", &s.calls)
            .finish()
    }
}

impl Forge for FakeForge {
    fn routes(&self) -> Vec<RouteRow> {
        let mut s = self.lock();
        s.calls.push(ForgeCall::Routes);
        s.routes.clone()
    }

    fn repo_of(&self, origin: &str) -> Option<ForgeRepo> {
        let mut s = self.lock();
        s.calls.push(ForgeCall::RepoOf(origin.to_owned()));
        s.repos.get(origin).cloned()
    }

    fn repo_token(&self, repo: &ForgeRepo) -> Result<Secret, ForgeError> {
        let mut s = self.lock();
        s.calls.push(ForgeCall::RepoToken(repo.clone()));
        match s.tokens.get(repo) {
            Some(TokenReply::Token(bytes)) => Ok(Secret::new(bytes.to_vec())),
            Some(TokenReply::Err(e)) => Err(e.clone()),
            None => Err(ForgeError::Failed(format!("no scripted token for {repo}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn repo() -> ForgeRepo {
        ForgeRepo::new("acme", "widgets")
    }

    #[test]
    fn routes_default_empty_and_scripted() {
        assert!(FakeForge::new().routes().is_empty());
        let fake = FakeForge::new().with_routes(vec![RouteRow::model()]);
        assert_eq!(fake.routes(), vec![RouteRow::model()]);
    }

    #[test]
    fn repo_of_scripted_and_unscripted() {
        let fake = FakeForge::new().with_repo("git@host:acme/widgets.git", repo());
        assert_eq!(fake.repo_of("git@host:acme/widgets.git"), Some(repo()));
        assert_eq!(fake.repo_of("git@host:other/thing.git"), None);
    }

    #[test]
    fn token_ok_gives_fresh_secret_each_call() {
        let fake = FakeForge::new().with_token(repo(), b"s3cret-token");
        let a = fake.repo_token(&repo()).expect("token");
        let b = fake.repo_token(&repo()).expect("token");
        assert_eq!(a.expose(), b"s3cret-token");
        assert_eq!(b.expose(), b"s3cret-token");
    }

    #[test]
    fn token_errors_per_variant() {
        for err in [
            ForgeError::Auth("denied".to_owned()),
            ForgeError::Network("down".to_owned()),
            ForgeError::Failed("boom".to_owned()),
        ] {
            let fake = FakeForge::new().with_token_error(repo(), err.clone());
            assert_eq!(fake.repo_token(&repo()).err(), Some(err));
        }
    }

    #[test]
    fn token_unscripted_repo_fails() {
        let err = FakeForge::new().repo_token(&repo()).err();
        assert_eq!(
            err,
            Some(ForgeError::Failed(
                "no scripted token for acme/widgets".to_owned()
            ))
        );
    }

    #[test]
    fn calls_recorded_in_order() {
        let fake = FakeForge::new();
        let _ = fake.repo_of("o");
        let _ = fake.routes();
        let _ = fake.repo_token(&repo());
        assert_eq!(
            fake.calls(),
            vec![
                ForgeCall::RepoOf("o".to_owned()),
                ForgeCall::Routes,
                ForgeCall::RepoToken(repo()),
            ]
        );
    }

    #[test]
    fn usable_as_trait_object() {
        let fake = Arc::new(FakeForge::new().with_token(repo(), b"t"));
        let dynamic: Arc<dyn Forge> = fake.clone();
        let by_ref: &dyn Forge = &*fake;
        assert_eq!(dynamic.repo_token(&repo()).expect("token").expose(), b"t");
        assert!(by_ref.routes().is_empty());
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn debug_redacts_token() {
        let fake = FakeForge::new().with_token(repo(), b"hunter2-token");
        let text = format!("{fake:?}");
        assert!(!text.contains("hunter2-token"));
    }
}
