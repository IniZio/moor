//! What the thread is shown when a turn ends.

use moor_core::secret::Secret;

use crate::escape::escape;

/// Longest reply posted, in characters.
const MAX_REPLY: usize = 4000;

/// What follows a reply that was cut.
const CUT_MARKER: &str = "… (cut)";

/// Longest plugin name shown, in characters.
const MAX_PLUGIN_NAME: usize = 80;

/// The line for a turn whose agent loaded the plugin `name`, which is not built in. The name
/// comes from the agent, so it is cut, escaped and shown without a backtick, which Slack cannot
/// escape inside a code span.
pub(super) fn plugin(name: &str) -> String {
    let name: String = name.chars().take(MAX_PLUGIN_NAME).collect();
    let name = escape(&name).replace('`', "'");
    format!("The coordinator did not start: Claude Code loaded the plugin `{name}`, which is not built in.")
}

/// The line for a turn that loaded more than the front gave it.
pub(super) const UNSAFE: &str = "The coordinator did not start safely.";

/// The line for a turn a restart cut off that does not run again.
pub(super) const RESTARTED: &str = "Stopped: the front restarted. Send your message again.";

/// The line for a turn that the front Proxy, which carries Claude's requests, could not serve.
pub(super) const PROXY_DOWN: &str =
    "The coordinator is unavailable: the Proxy that carries Claude's requests is not running, and moor is restarting it. Send your message again in a moment.";

/// Why a turn cannot run on a front the controller gave no Proxy.
pub(super) const NO_PROXY: &str =
    "the controller gave this front no Proxy; list `proxy_rules: [claude]` under its `fronts` entry";

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
