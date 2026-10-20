//! Host-side credential capture for the nono proxy.
//!
//! nono asks for each rule's credential (`cmd://<rule>`) on every request that
//! needs one. The value is resolved on the host, kept in memory for
//! [`CACHE_TTL`], and dropped when the upstream answers 401. Failed fetches are
//! never cached. Errors name the config key and never contain secret bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::seam::clock::Clock;
use moor_core::seam::forge::{Forge, ForgeRepo};
use moor_core::Secret;
use nono_proxy::capture::{
    CredentialCaptureBackend, CredentialCaptureError, CredentialCaptureMaterial,
    CredentialCaptureMetadata, CredentialCaptureRequest, CredentialCaptureResponse,
};
use zeroize::Zeroizing;

/// Where an inject form puts the secret.
const SECRET_MARK: &str = "{secret}";

/// Config key the forge token command comes from.
const FORGE_KEY: &str = "github.you_token_cmd";

/// Rule credential name that selects the forge repo token.
const FORGE_CREDENTIAL: &str = "forge";

/// How long a fetched credential is reused before the source is asked again.
pub(crate) const CACHE_TTL: Duration = Duration::from_secs(300);

/// Fetches one credential; error text names the config key only.
pub(crate) type FetchFn = Arc<dyn Fn() -> Result<Secret, String> + Send + Sync>;

/// Reads a named `credentials:` entry on the host.
pub(crate) type CredsFn = Arc<dyn Fn(&str) -> Result<Secret, String> + Send + Sync>;

/// How one rule's credential is obtained.
pub(crate) struct RuleSource {
    /// Config key the credential comes from; the only thing errors name.
    pub(crate) config_key: String,
    /// Fetches the credential.
    pub(crate) fetch: FetchFn,
    /// Basic-auth user; the response is then `user:secret`.
    pub(crate) basic_user: Option<String>,
}

/// Builds one source per resolved rule.
///
/// A `forge` credential mints a repo token through `forge`; any other name is
/// read through `creds`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "callers hand over their shared handles; the sources keep clones"
)]
pub(crate) fn sources_for(
    rules: &[ResolvedRule],
    creds: CredsFn,
    forge: Arc<dyn Forge>,
    repo: Option<ForgeRepo>,
) -> BTreeMap<String, RuleSource> {
    rules
        .iter()
        .map(|rule| {
            let (config_key, fetch) = if rule.credential == FORGE_CREDENTIAL {
                (FORGE_KEY.to_owned(), forge_fetch(&forge, repo.as_ref()))
            } else {
                let name = rule.credential.clone();
                let creds = Arc::clone(&creds);
                let fetch: FetchFn = Arc::new(move || creds(&name));
                (format!("credentials.{}", rule.credential), fetch)
            };
            let basic_user = match &rule.inject {
                ResolvedInject::BasicAuth { user } => Some(user.clone()),
                ResolvedInject::Header { .. } => None,
            };
            let source = RuleSource {
                config_key,
                fetch,
                basic_user,
            };
            (rule.name.clone(), source)
        })
        .collect()
}

/// Forge token fetch for `repo`; fails without a repo.
fn forge_fetch(forge: &Arc<dyn Forge>, repo: Option<&ForgeRepo>) -> FetchFn {
    let forge = Arc::clone(forge);
    let repo = repo.cloned();
    Arc::new(move || {
        let repo = repo
            .as_ref()
            .ok_or_else(|| format!("{FORGE_KEY}: the Box has no forge repo"))?;
        forge
            .repo_token(repo)
            .map_err(|_| format!("{FORGE_KEY} unavailable"))
    })
}

/// A cached secret and the instant it was fetched.
type Slot = (Secret, Instant);

/// nono credential capture backend; caches per rule for [`CACHE_TTL`].
pub(crate) struct MoorCapture {
    /// Credential source per rule name.
    sources: BTreeMap<String, RuleSource>,
    /// Source of the cache age.
    clock: Arc<dyn Clock>,
    /// Cached secrets by rule; never held while a source runs.
    cache: Mutex<BTreeMap<String, Slot>>,
    /// Rules whose evictions the GraphQL filter drives, with how each injects
    /// its credential; audit 401s are ignored for them.
    filtered: BTreeMap<String, ResolvedInject>,
}

impl MoorCapture {
    /// Builds a capture backend with an empty cache.
    pub(crate) fn new(sources: BTreeMap<String, RuleSource>, clock: Arc<dyn Clock>) -> Self {
        Self {
            sources,
            clock,
            cache: Mutex::new(BTreeMap::new()),
            filtered: BTreeMap::new(),
        }
    }

    /// Leaves evictions for `rules` to [`Self::evict_if_current`] callers: the
    /// audit drain cannot tell a credentialed 401 from an anonymous one for them.
    pub(crate) fn defer_evictions(
        &mut self,
        rules: impl IntoIterator<Item = (String, ResolvedInject)>,
    ) {
        self.filtered.extend(rules);
    }

    /// Drops the cached credential for `rule` on an audited upstream 401,
    /// unless the filter owns that rule's evictions.
    pub(crate) fn evict_audited(&self, rule: &str) {
        if !self.filtered.contains_key(rule) {
            self.evict(rule);
        }
    }

    /// Drops the cached credential for `rule` only if `presented` is exactly
    /// what nono injects for it now, so a forged header cannot force a refetch.
    pub(crate) fn evict_if_current(&self, rule: &str, presented: &str) {
        let Some(inject) = self.filtered.get(rule) else {
            return;
        };
        let mut cache = self.lock();
        let Some((secret, _)) = cache.get(rule) else {
            return;
        };
        let Ok(text) = std::str::from_utf8(secret.expose()) else {
            return;
        };
        let expected = Zeroizing::new(match inject {
            ResolvedInject::Header { form, .. } => form.replace(SECRET_MARK, text),
            ResolvedInject::BasicAuth { user } => {
                format!("Basic {}", base64(format!("{user}:{text}").as_bytes()))
            }
        });
        if ct_eq(expected.as_bytes(), presented.as_bytes()) {
            cache.remove(rule);
        }
    }

    /// Drops the cached credential for `rule` (called on an upstream 401).
    pub(crate) fn evict(&self, rule: &str) {
        self.lock().remove(rule);
    }

    /// Locks the cache, ignoring poisoning.
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Slot>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The secret for `rule` while it is younger than [`CACHE_TTL`], formatted.
    fn cached(&self, rule: &str, source: &RuleSource) -> Option<Result<Zeroizing<String>, ()>> {
        let now = self.clock.instant();
        let cache = self.lock();
        let (secret, at) = cache.get(rule)?;
        let fresh = now.saturating_duration_since(*at) < CACHE_TTL;
        let out = fresh.then(|| material(secret, source));
        drop(cache);
        out
    }

    /// Asks the source for a fresh secret and caches it.
    fn miss(&self, rule: &str, source: &RuleSource) -> Result<Zeroizing<String>, String> {
        let unavailable = || format!("{} unavailable", source.config_key);
        let fresh = (source.fetch)().map_err(|_| unavailable())?;
        if fresh.expose().trim_ascii().is_empty() {
            return Err(unavailable());
        }
        let text = material(&fresh, source).map_err(|()| unavailable())?;
        let now = self.clock.instant();
        self.lock().insert(rule.to_owned(), (fresh, now));
        Ok(text)
    }
}

/// Whether `a` and `b` are equal, without stopping at the first difference.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let diff = a.iter().zip(b).fold(0_u8, |d, (x, y)| d | (x ^ y));
    a.len() == b.len() && diff == 0
}

/// Standard padded base64.
pub(crate) fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0);
                out.push(char::from(ALPHABET.get(idx).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The text nono injects: the secret, or `user:secret` for basic auth.
///
/// Fails when the secret is not UTF-8.
fn material(secret: &Secret, source: &RuleSource) -> Result<Zeroizing<String>, ()> {
    let text = std::str::from_utf8(secret.expose()).map_err(|_| ())?;
    let mut out = Zeroizing::new(String::with_capacity(text.len() + 16));
    if let Some(user) = &source.basic_user {
        out.push_str(user);
        out.push(':');
    }
    out.push_str(text);
    Ok(out)
}

/// A capture response whose metadata says only hit or miss.
fn response(text: Zeroizing<String>, action: &str) -> CredentialCaptureResponse {
    CredentialCaptureResponse {
        material: CredentialCaptureMaterial::Secret(text),
        metadata: CredentialCaptureMetadata {
            cache_action: action.to_owned(),
            ..CredentialCaptureMetadata::default()
        },
    }
}

/// A capture error carrying `reason` and no other detail.
fn failure(reason: String) -> CredentialCaptureError {
    CredentialCaptureError::new(reason, CredentialCaptureMetadata::default())
}

impl CredentialCaptureBackend for MoorCapture {
    fn capture(
        &self,
        request: CredentialCaptureRequest,
    ) -> Result<CredentialCaptureResponse, CredentialCaptureError> {
        let rule = request.credential_name;
        let source = self
            .sources
            .get(&rule)
            .ok_or_else(|| failure(format!("unknown proxy rule {rule}")))?;
        match self.cached(&rule, source) {
            Some(Ok(text)) => return Ok(response(text, "hit")),
            Some(Err(())) => return Err(failure(format!("{} unavailable", source.config_key))),
            None => {}
        }
        self.miss(&rule, source)
            .map(|text| response(text, "miss"))
            .map_err(failure)
    }
}

impl fmt::Debug for MoorCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cache = self.lock();
        let rules: Vec<(&String, bool)> = self
            .sources
            .keys()
            .map(|name| (name, cache.contains_key(name)))
            .collect();
        f.debug_struct("MoorCapture")
            .field("rules_cached", &rules)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
