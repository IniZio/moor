//! Production source of the credential injected per route.
//!
//! The model token and the forge repo token are each fetched on demand and
//! kept in memory for [`TOKEN_TTL`]; a failed fetch is never cached. Errors
//! name the config key the value comes from and never contain token bytes.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use moor_core::proxy_gate::RouteId;
use moor_core::seam::clock::Clock;
use moor_core::seam::forge::{Forge, ForgeRepo};
use moor_core::Secret;

use super::upstream::{CredError, RouteCredential};

/// Source of the model token; production wraps the Creds reader, whose errors
/// are moor-worded and never contain the value.
pub(crate) type ModelToken = Arc<dyn Fn() -> Result<Secret, String> + Send + Sync>;

/// Config key the model token comes from.
pub(crate) const MODEL_SOURCE: &str = "credentials.CLAUDE_CODE_OAUTH_TOKEN";

/// Config key the forge token command comes from.
pub(crate) const FORGE_SOURCE: &str = "github.you_token_cmd";

/// How long a fetched token is reused before the source is asked again.
pub(crate) const TOKEN_TTL: Duration = Duration::from_secs(300);

/// A credential could not be resolved for a route.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ResolveError {
    /// The route has no credential source.
    #[error("no credential for route {0}")]
    UnknownRoute(String),
    /// A forge route was asked for but the Task has no forge repo.
    #[error("{FORGE_SOURCE}: the Task has no forge repo")]
    NoRepo,
    /// The source failed; `reason` is already free of token bytes.
    #[error("{origin}: {reason}")]
    Source {
        /// Config key the credential comes from.
        origin: &'static str,
        /// Why the source failed.
        reason: String,
    },
}

/// A token and the instant it was fetched.
type Slot = Option<(Secret, Instant)>;

/// Which cache slot a route reads.
#[derive(Clone, Copy)]
enum Group {
    /// The model token.
    Model,
    /// The forge repo token, shared by the git and API routes.
    Forge,
}

/// Cached tokens, one slot per group.
#[derive(Default)]
struct Cache {
    /// Model token slot.
    model: Slot,
    /// Forge token slot.
    forge: Slot,
}

impl Cache {
    /// The slot for `group`.
    const fn slot(&mut self, group: Group) -> &mut Slot {
        match group {
            Group::Model => &mut self.model,
            Group::Forge => &mut self.forge,
        }
    }
}

/// Resolves route credentials from the model and forge sources, with a short
/// in-memory cache.
pub(crate) struct Resolver {
    /// Reads the model token.
    model: ModelToken,
    /// Mints forge repo tokens.
    forge: Arc<dyn Forge>,
    /// Forge repo of the Task, if it has one.
    repo: Option<ForgeRepo>,
    /// Source of the cache age.
    clock: Arc<dyn Clock>,
    /// Cached tokens; never held while a source runs.
    cache: Mutex<Cache>,
}

impl Resolver {
    /// Builds a resolver with an empty cache.
    pub(crate) fn new(
        model: ModelToken,
        forge: Arc<dyn Forge>,
        repo: Option<ForgeRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            model,
            forge,
            repo,
            clock,
            cache: Mutex::new(Cache::default()),
        }
    }

    /// The token for `route`, from cache while younger than [`TOKEN_TTL`].
    pub(crate) fn resolve(&self, route: &RouteId) -> Result<Secret, ResolveError> {
        let group = match route.as_str() {
            "model" => Group::Model,
            "forge.git" | "forge.api" => {
                if self.repo.is_none() {
                    return Err(ResolveError::NoRepo);
                }
                Group::Forge
            }
            other => return Err(ResolveError::UnknownRoute(other.to_owned())),
        };
        if let Some(hit) = self.cached(group) {
            return Ok(hit);
        }
        let fresh = self.fetch(group)?;
        let copy = Secret::new(fresh.expose().to_vec());
        let now = self.clock.instant();
        *self.lock().slot(group) = Some((fresh, now));
        Ok(copy)
    }

    /// A copy of the cached token for `group` when it is still fresh.
    fn cached(&self, group: Group) -> Option<Secret> {
        let now = self.clock.instant();
        let mut cache = self.lock();
        let hit = {
            let (secret, at) = cache.slot(group).as_ref()?;
            (now.saturating_duration_since(*at) < TOKEN_TTL)
                .then(|| Secret::new(secret.expose().to_vec()))
        };
        drop(cache);
        hit
    }

    /// Asks the source for `group`; the lock is not held.
    fn fetch(&self, group: Group) -> Result<Secret, ResolveError> {
        match group {
            Group::Model => (self.model)().map_err(|reason| ResolveError::Source {
                origin: MODEL_SOURCE,
                reason,
            }),
            Group::Forge => {
                let repo = self.repo.as_ref().ok_or(ResolveError::NoRepo)?;
                self.forge
                    .repo_token(repo)
                    .map_err(|err| ResolveError::Source {
                        origin: FORGE_SOURCE,
                        reason: err.to_string(),
                    })
            }
        }
    }

    /// Locks the cache, ignoring poisoning.
    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RouteCredential for Resolver {
    /// Resolves `route`; the cause is dropped so no detail reaches the relay.
    fn credential(&self, route: &RouteId) -> Result<Secret, CredError> {
        self.resolve(route).map_err(|_| CredError)
    }
}

impl fmt::Debug for Resolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cache = self.lock();
        f.debug_struct("Resolver")
            .field("repo", &self.repo)
            .field("model_cached", &cache.model.is_some())
            .field("forge_cached", &cache.forge.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
