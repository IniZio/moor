//! The 403 refusal and the log line that names why.

use std::os::unix::net::UnixStream;
use std::sync::PoisonError;

use super::{refuse, Sink};

/// Where refusals are written and what they name.
pub(super) struct Refusals<'a> {
    /// Receives one line per refusal.
    pub(super) log: &'a Sink,
    /// The request's host and port, or a stand-in when the head did not parse.
    pub(super) target: String,
}

impl Refusals<'_> {
    /// Answers 403 and logs `reason`.
    pub(super) fn refuse(&self, client: UnixStream, reason: &str) {
        drop(writeln!(
            self.log.lock().unwrap_or_else(PoisonError::into_inner),
            "moor proxy: refused {}: {reason}",
            self.target
        ));
        refuse(client);
    }
}

/// The host and port a request head aims at, without scheme, userinfo, path or headers.
pub(super) fn target_of(head: &[u8]) -> String {
    let line = head.split(|b| *b == b'\r').next().unwrap_or_default();
    let line = String::from_utf8_lossy(line);
    let target = line.split(' ').nth(1).unwrap_or_default();
    let target = target.split_once("://").map_or(target, |(_, rest)| rest);
    let target = target.split(['/', '?']).next().unwrap_or_default();
    let target = target.rsplit('@').next().unwrap_or_default();
    if target.is_empty() || target.len() > 255 {
        "an unnamed target".to_owned()
    } else {
        target.to_owned()
    }
}
