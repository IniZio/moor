//! Signing in to a remote moor: the saved credentials, and the access token each dial
//! presents, refreshed under a file lock so concurrent plugin processes refresh once.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use moor_client::oidc::Http;
use moor_client::TokenSource;
use oauth2::basic::{BasicClient, BasicErrorResponseType};
use oauth2::{ClientId, RefreshToken, RequestTokenError, TokenResponse, TokenUrl};
use serde::{Deserialize, Serialize};

pub mod login;

/// Seconds before expiry from which an access token is refreshed.
const MARGIN: i64 = 300;

/// Lifetime assumed when the token endpoint names none.
const DEFAULT_LIFETIME: i64 = 3600;

/// What `login` saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// The moor server, without a trailing slash.
    pub server: String,
    /// The OIDC issuer the server named.
    pub issuer: String,
    /// The public client id the server named.
    pub client_id: String,
    /// Where refresh tokens are traded.
    pub token_endpoint: String,
    /// The long-lived grant.
    pub refresh_token: String,
    /// The cached bearer token.
    pub access_token: String,
    /// When the access token expires, in unix seconds.
    pub expires_at: i64,
}

/// Where the plugin keeps its state, which keeps its path across updates.
#[must_use]
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_STATE_DIR") {
        return dir.into();
    }
    let state = std::env::var_os("XDG_STATE_HOME").map_or_else(
        || Path::new(&std::env::var_os("HOME").unwrap_or_default()).join(".local/state"),
        PathBuf::from,
    );
    state.join("herdr/plugins/moor")
}

/// `server` as the credentials name it.
#[must_use]
pub fn normalize(server: &str) -> String {
    server.trim_end_matches('/').to_owned()
}

/// The credentials file and its lock in one directory.
#[derive(Debug, Clone)]
pub struct Store {
    /// The directory holding both files.
    dir: PathBuf,
}

impl Store {
    /// A store in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The credentials file.
    fn file(&self) -> PathBuf {
        self.dir.join("credentials.json")
    }

    /// The saved credentials, if any.
    ///
    /// # Errors
    /// The file exists and cannot be read.
    pub fn read(&self) -> Result<Option<Credentials>, String> {
        match fs::read(self.file()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
                format!(
                    "{} is unreadable: {e}; run `moor-herdr login`",
                    self.file().display()
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", self.file().display())),
        }
    }

    /// Saves `creds` with mode 0600, replacing the file in one step.
    ///
    /// # Errors
    /// The directory or file cannot be written.
    pub fn write(&self, creds: &Credentials) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self
            .dir
            .join(format!("credentials.json.{}.tmp", std::process::id()));
        let mut file = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec_pretty(creds)?)?;
        file.sync_all()?;
        fs::rename(&tmp, self.file())
    }

    /// Deletes the saved credentials; true when there were any.
    ///
    /// # Errors
    /// The file exists and cannot be removed.
    pub fn delete(&self) -> io::Result<bool> {
        match fs::remove_file(self.file()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Takes the exclusive refresh lock, waiting for another process to let go.
    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(self.dir.join("credentials.lock"))?;
        file.lock()?;
        Ok(file)
    }
}

/// Now, in unix seconds.
#[must_use]
#[expect(
    clippy::disallowed_methods,
    reason = "the one clock read of token expiry; tests pass their own"
)]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// The access token for one server, from the saved credentials.
#[derive(Debug)]
pub struct Tokens {
    /// Where the credentials are saved.
    store: Store,
    /// The server, normalized.
    server: String,
    /// Unix seconds now.
    clock: fn() -> i64,
}

impl Tokens {
    /// Tokens for `server` from `store`.
    #[must_use]
    pub fn new(store: Store, server: &str) -> Self {
        Self::with_clock(store, server, unix_now)
    }

    /// Tokens that read the time from `clock`.
    #[must_use]
    pub fn with_clock(store: Store, server: &str, clock: fn() -> i64) -> Self {
        Self {
            store,
            server: normalize(server),
            clock,
        }
    }

    /// The saved credentials for this server.
    fn saved(&self) -> Result<Credentials, String> {
        let creds = self
            .store
            .read()?
            .ok_or_else(|| "not signed in; run `moor-herdr login`".to_owned())?;
        if creds.server == self.server {
            Ok(creds)
        } else {
            Err(format!(
                "signed in to {}, not {}; run `moor-herdr login --server {}`",
                creds.server, self.server, self.server
            ))
        }
    }

    /// Whether `creds` has more than the refresh margin left.
    fn fresh(&self, creds: &Credentials) -> bool {
        creds.expires_at - (self.clock)() > MARGIN
    }

    /// Refreshes the saved credentials unless another process just did.
    fn refreshed(&self) -> Result<Credentials, String> {
        let _lock = self
            .store
            .lock()
            .map_err(|e| format!("cannot lock the credentials: {e}"))?;
        let creds = self.saved()?;
        if self.fresh(&creds) {
            return Ok(creds);
        }
        let next = refresh(&creds, (self.clock)())?;
        self.store
            .write(&next)
            .map_err(|e| format!("cannot save the credentials: {e}"))?;
        Ok(next)
    }
}

impl TokenSource for Tokens {
    fn bearer(&self) -> Result<String, String> {
        let creds = self.saved()?;
        if self.fresh(&creds) {
            return Ok(creds.access_token);
        }
        self.refreshed().map(|c| c.access_token)
    }
}

/// `creds` with the tokens the token endpoint returns for its refresh token.
fn refresh(creds: &Credentials, now: i64) -> Result<Credentials, String> {
    let token_url = TokenUrl::new(creds.token_endpoint.clone()).map_err(|e| e.to_string())?;
    let answer = BasicClient::new(ClientId::new(creds.client_id.clone()))
        .set_token_uri(token_url)
        .exchange_refresh_token(&RefreshToken::new(creds.refresh_token.clone()))
        .request(&Http::new());
    let token = match answer {
        Ok(token) => token,
        Err(RequestTokenError::ServerResponse(r))
            if *r.error() == BasicErrorResponseType::InvalidGrant =>
        {
            return Err("the sign-in ended; run `moor-herdr login`".to_owned());
        }
        Err(e) => {
            return Err(format!(
                "refreshing the sign-in at {} failed: {e}",
                creds.token_endpoint
            ))
        }
    };
    let lifetime = token.expires_in().map_or(DEFAULT_LIFETIME, |d| {
        i64::try_from(d.as_secs()).unwrap_or(DEFAULT_LIFETIME)
    });
    Ok(Credentials {
        access_token: token.access_token().secret().clone(),
        refresh_token: token
            .refresh_token()
            .map_or_else(|| creds.refresh_token.clone(), |t| t.secret().clone()),
        expires_at: now + lifetime,
        ..creds.clone()
    })
}
