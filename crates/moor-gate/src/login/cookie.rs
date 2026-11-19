//! The session cookie a sign-in, a renewal or a check hands the browser.

use std::time::Duration;

use super::routes;

/// A sealed session cookie and how long it lasts.
pub(crate) struct Issued {
    /// The cookie value.
    pub(super) sealed: String,
    /// Its life from now.
    pub(super) ttl: Duration,
}

impl Issued {
    /// Its life from now.
    pub(crate) const fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The `Set-Cookie` header line that stores it, kept by the browser across restarts.
    pub(crate) fn header(&self) -> String {
        format!(
            "Set-Cookie: {}={}; Max-Age={}; HttpOnly; Secure; SameSite=Lax; Path=/\r\n",
            routes::COOKIE,
            self.sealed,
            self.ttl.as_secs()
        )
    }
}
