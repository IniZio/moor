//! The Postgres database a store opens, from a `postgres://` URL: its connection settings, and
//! the URL with its password hidden for messages.

use std::fmt;
use std::time::Duration;

use postgres::{Client, Config, NoTls};
use url::Url;

/// The server option that makes every transaction on a connection read-only.
const READ_ONLY: &str = "-c default_transaction_read_only=on";

/// How long a connection attempt waits when the URL sets no `connect_timeout`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A Postgres database to connect to.
#[derive(Clone)]
pub(crate) struct Target {
    /// Connection settings.
    config: Config,
    /// The URL without its password or query, for messages.
    shown: String,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.shown)
    }
}

impl Target {
    /// The database `url` names, a `postgres://` or `postgresql://` URL.
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let parsed = Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
        if !matches!(parsed.scheme(), "postgres" | "postgresql") {
            return Err(format!(
                "the scheme is `{}`, not postgres or postgresql",
                parsed.scheme()
            ));
        }
        let shown = shown(&parsed);
        let mut config: Config = url.parse().map_err(|e| format!("{shown}: {e}"))?;
        if config.get_connect_timeout().is_none() {
            config.connect_timeout(CONNECT_TIMEOUT);
        }
        Ok(Self { config, shown })
    }

    /// This database for reads only: every transaction on its connections is read-only.
    pub(crate) fn read_only(&self) -> Self {
        let mut read_only = self.clone();
        let options = self.config.get_options().map_or_else(
            || READ_ONLY.to_owned(),
            |options| format!("{options} {READ_ONLY}"),
        );
        read_only.config.options(&options);
        read_only
    }

    /// A new connection.
    pub(super) fn connect(&self) -> Result<Client, postgres::Error> {
        self.config.connect(NoTls)
    }

    /// The URL without its password or query.
    pub(crate) fn shown(&self) -> &str {
        &self.shown
    }
}

/// `url` without its password or query, which may hold a password too.
fn shown(url: &Url) -> String {
    let mut shown = url.clone();
    shown.set_query(None);
    shown.set_fragment(None);
    if shown.password().is_some() {
        let _ = shown.set_password(Some("***"));
    }
    shown.into()
}
