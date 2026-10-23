//! Checks a pasted GitHub token before the front stores it: one `GET /user` that GitHub
//! must answer 200.

use moor_core::secret::Secret;
use ureq::Agent;

use crate::api::http_agent;
use crate::credentials::CheckError;

/// The `User-Agent` GitHub requires on every call.
const USER_AGENT: &str = "moor-slack";

/// A GitHub API client rooted at one base URL.
pub(crate) struct GitHub {
    /// Base URL without a trailing slash.
    base: String,
    /// HTTP client.
    agent: Agent,
}

impl GitHub {
    /// A client for `base`, such as `https://api.github.com`.
    pub(crate) fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: http_agent(),
        }
    }

    /// Why `token` cannot be what `gh auth token` prints, found without asking GitHub: it
    /// must be one visible ASCII word.
    pub(crate) fn check_format(token: &Secret) -> Result<(), String> {
        let bytes = token.expose();
        if bytes.is_empty() {
            return Err("it is empty".to_owned());
        }
        if !bytes.iter().all(u8::is_ascii_graphic) {
            return Err("it is not the one word `gh auth token` prints".to_owned());
        }
        Ok(())
    }

    /// Asks GitHub whose token `token` is. 200 takes it, 401 and 403 refuse it, and anything
    /// else leaves GitHub unasked.
    pub(crate) fn check(&self, token: &Secret) -> Result<(), CheckError> {
        let text = std::str::from_utf8(token.expose())
            .map_err(|_| CheckError::Refused("it is not text".to_owned()))?;
        let reply = self
            .agent
            .get(&format!("{}/user", self.base))
            .header("Authorization", format!("Bearer {text}"))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| CheckError::Unavailable(format!("could not reach GitHub: {e}")))?;
        match reply.status().as_u16() {
            200 => Ok(()),
            status @ (401 | 403) => Err(CheckError::Refused(format!(
                "GitHub answered HTTP {status} for it"
            ))),
            status => Err(CheckError::Unavailable(format!(
                "GitHub answered HTTP {status}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
