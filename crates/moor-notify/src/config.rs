//! The front's config file: strict parse plus validation, no IO beyond the secret files.

use std::fs::File;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use moor_core::config::parse_strict;
use moor_core::user::Email;
use serde::Deserialize;

/// The event kinds a hook gets when it names none: a turn ended, or a Box or its agent
/// stopped in a way someone must look at.
pub const DEFAULT_EVENTS: [&str; 6] = [
    "idled",
    "setup_failed",
    "session_failed",
    "launch_timed_out",
    "relaunch_capped",
    "box_failed",
];

/// Most bytes of one event kind name.
const KIND_MAX: usize = 64;

/// The `moor-notify` config file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// One entry per webhook.
    #[serde(default)]
    hooks: Vec<HookFile>,
}

/// One webhook as written in the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookFile {
    /// The moor user whose Boxes the hook follows, by the email the controller knows them by.
    email: String,
    /// Where each event is posted.
    url: String,
    /// Owner-only file holding the HMAC key.
    secret_file: PathBuf,
    /// Event kinds to post; [`DEFAULT_EVENTS`] when absent.
    #[serde(default)]
    events: Option<Vec<String>>,
}

/// One validated webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// The user the hook follows.
    pub email: Email,
    /// The `https://` (or loopback `http://`) URL each event is posted to.
    pub url: String,
    /// The HMAC key.
    pub secret: Vec<u8>,
    /// Event kinds posted, deduplicated, in the order given.
    pub events: Vec<String>,
}

/// Parses and checks the config at `path`, reading each hook's secret file.
///
/// # Errors
/// A sentence naming the file and what is wrong with it.
pub fn load(path: &Path) -> Result<Vec<Hook>, String> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    parse(&shown, &text, read_secret)
}

/// Parses `text` from `file`, reading each secret with `secret`.
pub(crate) fn parse(
    file: &str,
    text: &str,
    secret: impl Fn(&Path) -> Result<Vec<u8>, String>,
) -> Result<Vec<Hook>, String> {
    let parsed: ConfigFile = parse_strict(file, text).map_err(|e| e.to_string())?;
    if parsed.hooks.is_empty() {
        return Err(format!("{file}: hooks: list at least one hook"));
    }
    parsed
        .hooks
        .into_iter()
        .enumerate()
        .map(|(i, hook)| hook_of(hook, &secret).map_err(|why| format!("{file}: hooks[{i}]: {why}")))
        .collect()
}

/// Checks one hook.
fn hook_of(
    hook: HookFile,
    secret: &impl Fn(&Path) -> Result<Vec<u8>, String>,
) -> Result<Hook, String> {
    let email = Email::parse(&hook.email)?;
    check_url(&hook.url)?;
    let events = match hook.events {
        None => DEFAULT_EVENTS.iter().map(|&k| k.to_owned()).collect(),
        Some(list) if list.is_empty() => {
            return Err("events: list at least one kind, or leave it out".to_owned())
        }
        Some(list) => dedup(list)?,
    };
    Ok(Hook {
        email,
        url: hook.url,
        secret: secret(&hook.secret_file)?,
        events,
    })
}

/// `kinds` without repeats, each checked to be a `snake_case` event kind.
fn dedup(kinds: Vec<String>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for kind in kinds {
        let shaped = kind.len() <= KIND_MAX
            && kind.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
            && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
        if !shaped {
            return Err(format!("events: `{kind}` is not an event kind"));
        }
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    Ok(out)
}

/// An `https://` URL, or an `http://` one on the loopback for a receiver on the same host.
fn check_url(url: &str) -> Result<(), String> {
    let parsed: ureq::http::Uri = url
        .parse()
        .map_err(|_| format!("url: `{url}` is not a URL"))?;
    let host = parsed.host().unwrap_or_default();
    match parsed.scheme_str() {
        Some("https") if !host.is_empty() => Ok(()),
        Some("http") if matches!(host, "127.0.0.1" | "localhost" | "[::1]") => Ok(()),
        _ => Err(format!(
            "url: `{url}` must be https://, or http:// on 127.0.0.1, [::1] or localhost"
        )),
    }
}

/// Reads an HMAC key from `path`, which must be owner-only and hold a non-empty line.
fn read_secret(path: &Path) -> Result<Vec<u8>, String> {
    let fail = |why: &dyn std::fmt::Display| format!("secret_file {}: {why}", path.display());
    let mut file = File::open(path).map_err(|e| fail(&e))?;
    let mode = file.metadata().map_err(|e| fail(&e))?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(fail(&"must be mode 0600"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|e| fail(&e))?;
    let key = text.trim_end();
    if key.is_empty() {
        return Err(fail(&"is empty"));
    }
    Ok(key.as_bytes().to_vec())
}

#[cfg(test)]
mod tests;
