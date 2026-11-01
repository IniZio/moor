//! Where the bearer token comes from: a shell command, or the shared moor sign-in.

use std::process::{Command, Stdio};

use moor_client::session::{normalize, Store, Tokens};
use moor_client::TokenSource;

/// Why a request did not get through.
#[derive(Debug)]
pub enum Fail {
    /// The gate refused the token (401 or 403).
    Rejected,
    /// There is no sign-in to the named server.
    SignedOut(String),
    /// Anything else, in words for a log line.
    Other(String),
}

impl Fail {
    /// Wraps `why` as an ordinary failure.
    pub(crate) fn other(why: &impl ToString) -> Self {
        Self::Other(why.to_string())
    }
}

/// A source of bearer tokens for the gate.
pub trait Bearer: Send + Sync {
    /// The token to connect with.
    fn bearer(&self) -> Result<String, Fail>;

    /// A new token, asked for after the gate refused the last one.
    fn renew(&self) -> Result<String, Fail>;
}

/// The token a shell command prints; it runs afresh on every ask and its output is never logged.
pub struct Shell(pub String);

impl Bearer for Shell {
    fn bearer(&self) -> Result<String, Fail> {
        let out = Command::new("sh")
            .args(["-c", &self.0])
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| Fail::other(&e))?;
        let token = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if out.status.success() && !token.is_empty() {
            Ok(token)
        } else {
            Err(Fail::other(&"the token command gave no token"))
        }
    }

    fn renew(&self) -> Result<String, Fail> {
        self.bearer()
    }
}

/// The saved moor sign-in to one server, refreshed when it runs out.
pub struct Session {
    /// Where sign-ins are saved.
    store: Store,
    /// The server's origin.
    server: String,
}

impl Session {
    /// The sign-in to `server` (an origin or any URL on it) saved in `store`.
    #[must_use]
    pub fn new(store: Store, server: &str) -> Self {
        Self {
            store,
            server: normalize(server),
        }
    }

    /// Fails with [`Fail::SignedOut`] unless there is a sign-in.
    fn signed_in(&self) -> Result<Tokens, Fail> {
        match self.store.read(&self.server) {
            Ok(Some(_)) => Ok(Tokens::new(self.store.clone(), &self.server)),
            Ok(None) => Err(Fail::SignedOut(self.server.clone())),
            Err(why) => Err(Fail::Other(why)),
        }
    }
}

impl Bearer for Session {
    fn bearer(&self) -> Result<String, Fail> {
        self.signed_in()?.bearer().map_err(Fail::Other)
    }

    fn renew(&self) -> Result<String, Fail> {
        self.signed_in()?.renew().map_err(Fail::Other)
    }
}
