//! How the controller signs a user in to one of their own MCP servers over OAuth.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The `oauth:` section of the operator's config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OAuthConfig {
    /// The redirect URI sign-ins use in place of the loopback address.
    pub callback_url: Option<String>,
    /// Clients registered ahead of time, by authorization server host; others register
    /// themselves.
    pub clients: BTreeMap<String, OAuthClient>,
}

/// A client the operator registered with one authorization server. Users never see it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthClient {
    /// The client id.
    pub client_id: String,
    /// The client secret, for a confidential client.
    #[serde(default)]
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClient")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// What the authorization server sent to the redirect URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    /// The `state` the sign-in started with.
    pub state: String,
    /// The authorization code.
    pub code: String,
    /// The issuer, when the server named one.
    pub iss: Option<String>,
}

impl OAuthCallback {
    /// Reads the address the browser ended on after signing in.
    ///
    /// # Errors
    /// The address is not a URL, reports an `error`, or lacks `state` or `code`.
    pub fn from_url(address: &str) -> Result<Self, String> {
        let url = url::Url::parse(address.trim())
            .map_err(|e| format!("`{address}` is not an address: {e}"))?;
        let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        if let Some(error) = query.get("error") {
            return Err(format!("the sign-in was refused: {error}"));
        }
        let field = |k: &str| {
            query
                .get(k)
                .cloned()
                .ok_or_else(|| format!("the address has no `{k}`"))
        };
        Ok(Self {
            state: field("state")?,
            code: field("code")?,
            iss: query.get("iss").cloned(),
        })
    }
}
