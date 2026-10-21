//! What the thread is shown when a turn ends.

use moor_core::secret::Secret;

use crate::escape::escape;

/// Longest reply posted, in characters.
const MAX_REPLY: usize = 4000;

/// What follows a reply that was cut.
const CUT_MARKER: &str = "… (cut)";

/// The line for a turn that loaded more than the front gave it.
pub(super) const UNSAFE: &str = "The coordinator did not start safely.";

/// The line for a turn killed at its limit.
pub(super) const KILLED: &str = "Stopped after 10 minutes.";

/// The coordinator's reply, escaped and cut to [`MAX_REPLY`] characters.
pub(super) fn reply(text: &str) -> String {
    let text = escape(text.trim());
    if text.chars().count() <= MAX_REPLY {
        return text;
    }
    let keep = MAX_REPLY - CUT_MARKER.chars().count();
    text.chars().take(keep).collect::<String>() + CUT_MARKER
}

/// The line for a starter who linked no credential.
pub(super) fn no_token(starter: &str) -> String {
    format!("<@{starter}> run `/moor login` first.")
}

/// The line for a credential the agent's provider refused.
pub(super) fn refused(starter: &str, label: &str) -> String {
    format!("<@{starter}>, {label} refused your token. Run `/moor login`.")
}

/// The line for a turn that ended in an error.
pub(super) fn stopped(first_line: &str) -> String {
    format!("The coordinator stopped: {}.", escape(first_line))
}

/// The line for a coordinator that cannot run.
pub(super) fn unavailable(why: &str) -> String {
    format!("The coordinator is unavailable: {}.", escape(why))
}

/// `text` with `credential` replaced wherever it appears.
pub(super) fn redact(text: &str, credential: &Secret) -> String {
    let credential = String::from_utf8_lossy(credential.expose());
    let credential = credential.trim();
    if credential.is_empty() {
        text.to_owned()
    } else {
        text.replace(credential, "[token]")
    }
}

#[cfg(test)]
mod tests;
