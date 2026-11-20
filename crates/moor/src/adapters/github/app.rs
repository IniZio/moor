//! The operator's GitHub App: its JWT, installation tokens, and the reads moor makes with them.
//!
//! moor only reads as the App. Minting an installation token is the one `POST`, and it asks for
//! read permissions only. No comment, status or other write goes out with an App token.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use moor_core::config::GitHubAppConfig;
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::Secret;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

/// GitHub's REST API.
const API: &str = "https://api.github.com";

/// How long one request may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Largest reply body read.
const REPLY_CAP: u64 = 8 * 1024 * 1024;

/// How far back a JWT's `iat` is set, for clock drift against GitHub.
const JWT_BACKDATE_SECS: i64 = 60;

/// How long a JWT lives; GitHub refuses one over 10 minutes.
const JWT_TTL_SECS: i64 = 9 * 60;

/// Most installations, and repos per installation, listed.
const PAGE: u32 = 100;

/// The permissions an installation token asks for: reads only.
const TOKEN_PERMISSIONS: &str = r#"{"permissions":{"metadata":"read","pull_requests":"read"}}"#;

/// The operator's GitHub App, from the controller config's `github_app` entry.
pub(crate) struct AppClient {
    /// The App id, the JWT's issuer.
    id: u64,
    /// The App's private key.
    key: EncodingKey,
    /// The REST API root, without a trailing slash.
    api: String,
    /// The App's public install page, read once from `GET /app`.
    install_url: OnceLock<String>,
}

/// An installation of the App on an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct Installation {
    /// GitHub's installation id.
    pub(crate) id: u64,
}

/// What the portal shows of the App.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Installed {
    /// The App's public install page.
    pub(crate) install_url: String,
    /// The repos the App is installed on.
    pub(crate) repos: Vec<ForgeRepo>,
}

impl AppClient {
    /// The App `config` names, with its key read from the file. `expand` resolves `~`.
    ///
    /// # Errors
    /// The key file cannot be read, or the key is not an RSA PEM. The error names the file,
    /// never its content.
    pub(crate) fn load(
        config: &GitHubAppConfig,
        expand: impl Fn(&Path) -> std::path::PathBuf,
    ) -> Result<Self, String> {
        let path = expand(&config.private_key_file);
        let pem = Zeroizing::new(
            fs::read(&path)
                .map_err(|e| format!("github_app: cannot read {}: {e}", path.display()))?,
        );
        Self::from_pem(config.id, &pem, API)
            .map_err(|e| format!("github_app: {}: {e}", path.display()))
    }

    /// The App `id` with the PKCS#1 PEM key `pem` GitHub generates, calling the REST API at
    /// `api`.
    ///
    /// # Errors
    /// `pem` is not an `RSA PRIVATE KEY` PEM block.
    pub(crate) fn from_pem(id: u64, pem: &[u8], api: &str) -> Result<Self, String> {
        let refused = || "not an RSA PRIVATE KEY PEM, as GitHub generates".to_owned();
        let block = pem::parse(pem).map_err(|_| refused())?;
        if block.tag() != "RSA PRIVATE KEY" {
            return Err(refused());
        }
        let der = Zeroizing::new(block.into_contents());
        let app = Self {
            id,
            key: EncodingKey::from_rsa_der(&der),
            api: api.trim_end_matches('/').to_owned(),
            install_url: OnceLock::new(),
        };
        // The key is only parsed when it signs, so sign once now to refuse a broken one here.
        app.jwt(Ts::from_millis(0)).map_err(|_| refused())?;
        Ok(app)
    }

    /// A JWT that authenticates as the App at `now`.
    fn jwt(&self, now: Ts) -> Result<Zeroizing<String>, String> {
        /// The claims GitHub reads.
        #[derive(Serialize)]
        struct Claims {
            /// Issued at, backdated for drift.
            iat: i64,
            /// Expiry.
            exp: i64,
            /// The App id.
            iss: String,
        }
        let secs = now.as_millis() / 1000;
        let claims = Claims {
            iat: secs - JWT_BACKDATE_SECS,
            exp: secs + JWT_TTL_SECS,
            iss: self.id.to_string(),
        };
        encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map(|t| Zeroizing::new(format!("Bearer {t}")))
            .map_err(|_| "cannot sign the App JWT".to_owned())
    }

    /// Sends one request with `auth` and returns the status and parsed JSON body.
    fn call(
        &self,
        method: &str,
        path: &str,
        auth: &str,
        body: Option<&str>,
    ) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.api);
        let agent = super::agent(TIMEOUT);
        let unavailable = |e: ureq::Error| format!("cannot reach GitHub: {e}");
        let response = if method == "POST" {
            agent
                .post(&url)
                .header("Authorization", auth)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "moor")
                .header("Content-Type", "application/json")
                .send(body.unwrap_or("{}"))
        } else {
            agent
                .get(&url)
                .header("Authorization", auth)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "moor")
                .call()
        };
        let mut response = response.map_err(unavailable)?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(REPLY_CAP + 1)
            .read_to_vec()
            .map_err(unavailable)?;
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, value))
    }

    /// A `GET` of `path` that must answer `200`, parsed as `T`.
    fn get<T: for<'de> Deserialize<'de>>(&self, path: &str, auth: &str) -> Result<T, String> {
        match self.call("GET", path, auth, None)? {
            (200, value) => serde_json::from_value(value)
                .map_err(|_| format!("GitHub answered {path} in an unknown shape")),
            (status, _) => Err(format!("GitHub answered {path} with HTTP {status}")),
        }
    }

    /// The App's public install page, `<html_url>/installations/new`.
    ///
    /// # Errors
    /// GitHub cannot be reached or refuses the App JWT.
    pub(crate) fn install_url(&self, now: Ts) -> Result<String, String> {
        /// `GET /app`.
        #[derive(Deserialize)]
        struct App {
            /// `https://github.com/apps/<slug>`.
            html_url: String,
        }
        if let Some(url) = self.install_url.get() {
            return Ok(url.clone());
        }
        let app: App = self.get("/app", &self.jwt(now)?)?;
        let url = format!("{}/installations/new", app.html_url.trim_end_matches('/'));
        Ok(self.install_url.get_or_init(|| url).clone())
    }

    /// The installation that covers `repo`, or none when the App is not installed there.
    ///
    /// # Errors
    /// GitHub cannot be reached or answers anything but `200` or `404`.
    pub(crate) fn installation_of(
        &self,
        repo: &ForgeRepo,
        now: Ts,
    ) -> Result<Option<Installation>, String> {
        let path = format!("/repos/{}/{}/installation", repo.owner, repo.name);
        match self.call("GET", &path, &self.jwt(now)?, None)? {
            (200, value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|_| format!("GitHub answered {path} in an unknown shape")),
            (404, _) => Ok(None),
            (status, _) => Err(format!("GitHub answered {path} with HTTP {status}")),
        }
    }

    /// A read-only token for `installation`, good for under an hour.
    ///
    /// # Errors
    /// GitHub cannot be reached or refuses the mint.
    pub(crate) fn token(&self, installation: Installation, now: Ts) -> Result<Secret, String> {
        /// The mint's reply.
        #[derive(Deserialize)]
        struct Minted {
            /// The token.
            token: String,
        }
        let path = format!("/app/installations/{}/access_tokens", installation.id);
        match self.call("POST", &path, &self.jwt(now)?, Some(TOKEN_PERMISSIONS))? {
            (201, value) => serde_json::from_value::<Minted>(value)
                .map(|m| Secret::new(m.token.into_bytes()))
                .map_err(|_| format!("GitHub answered {path} in an unknown shape")),
            (status, _) => Err(format!("GitHub answered {path} with HTTP {status}")),
        }
    }

    /// The App and the repos it is installed on, at most [`PAGE`] installations and [`PAGE`]
    /// repos each.
    ///
    /// # Errors
    /// GitHub cannot be reached or refuses a read.
    pub(crate) fn installed(&self, now: Ts) -> Result<Installed, String> {
        /// `GET /installation/repositories`.
        #[derive(Deserialize)]
        struct Repos {
            /// The repos.
            repositories: Vec<Repo>,
        }
        /// One repo.
        #[derive(Deserialize)]
        struct Repo {
            /// `owner/name`.
            full_name: String,
        }
        let install_url = self.install_url(now)?;
        let path = format!("/app/installations?per_page={PAGE}");
        let installations: Vec<Installation> = self.get(&path, &self.jwt(now)?)?;
        let mut repos = Vec::new();
        for installation in installations {
            let token = self.token(installation, now)?;
            let path = format!("/installation/repositories?per_page={PAGE}");
            let listed: Repos = self.get(&path, &bearer(&token)?)?;
            repos.extend(
                listed
                    .repositories
                    .iter()
                    .filter_map(|r| ForgeRepo::parse(&r.full_name).ok()),
            );
        }
        repos.sort_by(|a, b| (&a.owner, &a.name).cmp(&(&b.owner, &b.name)));
        repos.dedup();
        Ok(Installed { install_url, repos })
    }
}

/// The `Authorization` value for an installation token; a token that is not printable ASCII is
/// refused unsent.
fn bearer(token: &Secret) -> Result<Zeroizing<String>, String> {
    let raw = token.expose();
    if raw.is_empty() || !raw.iter().all(u8::is_ascii_graphic) {
        return Err("the installation token is not printable ASCII".to_owned());
    }
    let mut auth = Zeroizing::new(String::with_capacity(raw.len() + 7));
    auth.push_str("Bearer ");
    auth.extend(raw.iter().map(|&b| char::from(b)));
    Ok(auth)
}

#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod pull;

#[cfg(test)]
mod tests;
