//! One sign-in per person per moor server, shared by every local moor client: saved in the
//! person's state directory, its access token refreshed under a file lock so concurrent
//! processes refresh once.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use oauth2::url::Url;
use serde::{Deserialize, Serialize};

pub use tokens::{login, logout, unix_now, Tokens};

#[cfg(test)]
mod tests;
mod tokens;

/// One server's sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// The moor server's origin.
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
    /// Who signed in, from the id token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// The credentials file: every server the person signed in to.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// Sign-ins by normalized server.
    #[serde(default)]
    servers: BTreeMap<String, Credentials>,
}

/// `server`, given as an origin or any URL on it, as the origin that keys its sign-in.
#[must_use]
pub fn normalize(server: &str) -> String {
    Url::parse(server).map_or_else(
        |_| server.trim_end_matches('/').to_owned(),
        |url| url.origin().ascii_serialization(),
    )
}

/// The credentials file and its lock in one directory.
#[derive(Debug, Clone)]
pub struct Store {
    /// The directory holding both files.
    dir: PathBuf,
    /// A credentials file of the old one-server layout to import a sign-in from.
    legacy: Option<PathBuf>,
}

/// `$XDG_STATE_HOME`, or `~/.local/state`.
fn xdg_state() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map_or_else(
        || Path::new(&std::env::var_os("HOME").unwrap_or_default()).join(".local/state"),
        PathBuf::from,
    )
}

impl Store {
    /// A store in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            legacy: None,
        }
    }

    /// The person's store, `$XDG_STATE_HOME/moor`, which imports a sign-in the herdr plugin
    /// saved before it shared this one.
    #[must_use]
    pub fn user() -> Self {
        let herdr = std::env::var_os("HERDR_PLUGIN_STATE_DIR")
            .map_or_else(|| xdg_state().join("herdr/plugins/moor"), PathBuf::from);
        Self {
            dir: xdg_state().join("moor"),
            legacy: Some(herdr.join("credentials.json")),
        }
    }

    /// The credentials file.
    fn file(&self) -> PathBuf {
        self.dir.join("credentials.json")
    }

    /// Every saved sign-in.
    fn load(&self) -> Result<Saved, String> {
        match fs::read(self.file()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                format!(
                    "{} is unreadable: {e}; sign in again",
                    self.file().display()
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Saved::default()),
            Err(e) => Err(format!("{}: {e}", self.file().display())),
        }
    }

    /// The sign-in to `server`, if any.
    ///
    /// # Errors
    /// The file exists and cannot be read.
    pub fn read(&self, server: &str) -> Result<Option<Credentials>, String> {
        let server = normalize(server);
        if let Some(creds) = self.load()?.servers.remove(&server) {
            return Ok(Some(creds));
        }
        self.import(&server)
    }

    /// Every saved sign-in, by server.
    ///
    /// # Errors
    /// The file exists and cannot be read.
    pub fn all(&self) -> Result<Vec<Credentials>, String> {
        Ok(self.load()?.servers.into_values().collect())
    }

    /// Copies the old layout's sign-in to `server` into this store, once.
    fn import(&self, server: &str) -> Result<Option<Credentials>, String> {
        let Some(old) = self.legacy.as_ref().and_then(|p| fs::read(p).ok()) else {
            return Ok(None);
        };
        let Ok(creds) = serde_json::from_slice::<Credentials>(&old) else {
            return Ok(None);
        };
        if normalize(&creds.server) != server {
            return Ok(None);
        }
        let _lock = self.lock().map_err(|e| e.to_string())?;
        let mut saved = self.load()?;
        let kept = saved
            .servers
            .entry(server.to_owned())
            .or_insert(creds)
            .clone();
        self.save(&saved).map_err(|e| e.to_string())?;
        if let Some(old) = &self.legacy {
            let _ = fs::rename(old, old.with_extension("json.imported"));
        }
        Ok(Some(kept))
    }

    /// Saves `creds` under its server with mode 0600, replacing the file in one step.
    ///
    /// # Errors
    /// The file cannot be read, or the directory or file cannot be written.
    pub fn write(&self, creds: &Credentials) -> io::Result<()> {
        let _lock = self.lock()?;
        self.put(creds)
    }

    /// [`Store::write`] for a caller that holds the lock.
    fn put(&self, creds: &Credentials) -> io::Result<()> {
        let mut saved = self.load().map_err(io::Error::other)?;
        saved
            .servers
            .insert(normalize(&creds.server), creds.clone());
        self.save(&saved)
    }

    /// Writes `saved` through a temporary file.
    fn save(&self, saved: &Saved) -> io::Result<()> {
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
        file.write_all(&serde_json::to_vec_pretty(saved)?)?;
        file.sync_all()?;
        fs::rename(&tmp, self.file())
    }

    /// Deletes the sign-in to `server`; true when there was one.
    ///
    /// # Errors
    /// The file cannot be read or written.
    pub fn delete(&self, server: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        let mut saved = self.load().map_err(io::Error::other)?;
        let gone = saved.servers.remove(&normalize(server)).is_some();
        if gone {
            self.save(&saved)?;
        }
        Ok(gone)
    }

    /// Takes the exclusive lock, waiting for another process to let go.
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
