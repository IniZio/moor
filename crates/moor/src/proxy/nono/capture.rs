//! Host-side credential capture for the nono proxy.
//!
//! nono asks for each rule's credential (`cmd://<rule>`) on every request that
//! needs one. Each ask reads the Box user's secret from the vault again, so a
//! secret that is set, changed or deleted counts from the next request on.
//! Errors name the secret and never contain its bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::user::{SecretName, UserId};
use moor_core::Secret;
use nono_proxy::capture::{
    CredentialCaptureBackend, CredentialCaptureError, CredentialCaptureMaterial,
    CredentialCaptureMetadata, CredentialCaptureRequest, CredentialCaptureResponse,
};
use zeroize::Zeroizing;

use crate::vault::Vault;

/// Fetches one credential; error text names the secret only.
pub(crate) type FetchFn = Arc<dyn Fn() -> Result<Secret, String> + Send + Sync>;

/// Reads `user`'s secret `name` with the vault key; `Ok(None)` when the user has none.
pub(crate) type SecretsFn =
    Arc<dyn Fn(&Vault, UserId, &SecretName) -> Result<Option<Secret>, String> + Send + Sync>;

/// How one rule's credential is obtained.
pub(crate) struct RuleSource {
    /// What the credential is called; the only thing errors name.
    pub(crate) config_key: String,
    /// Fetches the credential.
    pub(crate) fetch: FetchFn,
    /// Basic-auth user; the response is then `user:secret`.
    pub(crate) basic_user: Option<String>,
}

/// Builds one source per resolved rule, each reading `user`'s secret that the rule names.
pub(crate) fn sources_for(
    rules: &[ResolvedRule],
    secrets: &SecretsFn,
    vault: &Arc<Vault>,
    user: UserId,
) -> BTreeMap<String, RuleSource> {
    rules
        .iter()
        .map(|rule| {
            let config_key = format!("secret {}", rule.credential);
            let name = SecretName::parse(&rule.credential);
            let (secrets, vault, key) =
                (Arc::clone(secrets), Arc::clone(vault), config_key.clone());
            let fetch: FetchFn = Arc::new(move || {
                let name = name.as_ref().map_err(Clone::clone)?;
                secrets(&vault, user, name)?.ok_or_else(|| format!("{key} is not set"))
            });
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

/// nono credential capture backend; asks the source on every request and keeps nothing.
pub(crate) struct MoorCapture {
    /// Credential source per rule name.
    sources: BTreeMap<String, RuleSource>,
}

impl MoorCapture {
    /// Builds a capture backend over `sources`.
    pub(crate) const fn new(sources: BTreeMap<String, RuleSource>) -> Self {
        Self { sources }
    }

    /// Asks the source for the secret, formatted for injection.
    fn fetch(source: &RuleSource) -> Result<Zeroizing<String>, String> {
        let unavailable = || format!("{} unavailable", source.config_key);
        let fresh = (source.fetch)().map_err(|_| unavailable())?;
        if fresh.expose().trim_ascii().is_empty() {
            return Err(unavailable());
        }
        material(&fresh, source).map_err(|()| unavailable())
    }
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

/// A capture response; every fetch is fresh, so its metadata says miss.
fn response(text: Zeroizing<String>) -> CredentialCaptureResponse {
    CredentialCaptureResponse {
        material: CredentialCaptureMaterial::Secret(text),
        metadata: CredentialCaptureMetadata {
            cache_action: "miss".to_owned(),
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
        Self::fetch(source).map(response).map_err(failure)
    }
}

impl fmt::Debug for MoorCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoorCapture")
            .field("rules", &self.sources.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests;
