//! Reads one file in a Box over ssh, never through `crabbox run`.
//!
//! `crabbox run` holds the Box's workspace for the whole command, so a read
//! issued through it would wait for a running command. `crabbox ssh` only
//! prints the ssh command line for the Box, so the read goes straight to ssh.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::errors::{map_error, Op};
use super::{link, split_handle, target_args, Crabbox, LOOKUP_DEADLINE};

/// Time allowed for the read.
const READ_DEADLINE: Duration = Duration::from_secs(5);
/// Remote exit code that means the file does not exist.
const MISSING: i32 = 44;

/// Reads `path` in the Box behind `handle`; `None` when the file is absent.
pub(super) fn peek_file(
    cb: &Crabbox,
    handle: &BoxHandle,
    path: &BoxPath,
) -> Result<Option<String>, DriverError> {
    let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    let p = quote(path.as_str());
    let remote = format!("test -e {p} || exit {MISSING}; cat -- {p}");
    let read = ProcSpec::new(Tool::Ssh, Deadline(READ_DEADLINE))
        .args(words)
        .arg(remote);
    match link::run(cb, handle, &read) {
        Ok(out) => Ok(Some(out.stdout)),
        Err(ProcError::NonZero { code: MISSING, .. }) => Ok(None),
        Err(ProcError::Timeout { after, .. }) => Err(DriverError::Timeout(after)),
        Err(ProcError::Spawn(_)) => Err(DriverError::Unavailable(
            "ssh could not be started".to_owned(),
        )),
        Err(e) => Err(
            link::lost(cb, handle, &e).unwrap_or_else(|| DriverError::Failed {
                summary: "box file read failed".to_owned(),
            }),
        ),
    }
}

/// The ssh arguments that reach the Box behind `handle`, without the leading `ssh`.
pub(super) fn ssh_target(cb: &Crabbox, handle: &BoxHandle) -> Result<Vec<String>, DriverError> {
    // Lookups are reused within one command and, for a while, through the Box's link, and
    // idle Boxes are slept by moor's `sleep_after`, not by crabbox's idle timer.
    let key = handle.as_str().to_owned();
    if let Some(words) = cb.targets.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(words);
    }
    if let Some(words) = cb.link(handle).and_then(|l| l.saved(cb.clock.now())) {
        if let Ok(mut map) = cb.targets.lock() {
            map.insert(key, words.clone());
        }
        return Ok(words);
    }
    lookup(cb, handle)
}

/// Asks crabbox for the ssh arguments of `handle` and keeps them for later calls. A lookup
/// also touches the Box's lease, which resets crabbox's idle timer.
pub(super) fn lookup(cb: &Crabbox, handle: &BoxHandle) -> Result<Vec<String>, DriverError> {
    // Secret-token providers need --show-secret, not yet passed.
    let (provider, _) = split_handle(handle);
    let mut spec = Crabbox::spec(cb.lookup_deadline(provider.unwrap_or_default())).arg("ssh");
    if cb.ssh_skips_probe() {
        spec = spec.arg("--no-probe");
    }
    let lookup = cb.at_lease(spec.args(target_args(handle)), handle)?;
    // Another crabbox call on the same lease can rewrite its claim mid-lookup.
    let line = cb
        .retry_claim_race(|| cb.proc.run(&lookup))
        .map_err(|e| map_error(Op::Target, provider, &e))?
        .stdout;
    match split_words(&line) {
        Some(words) if words.first().is_some_and(|w| w == "ssh") => {
            let words: Vec<String> = words.into_iter().skip(1).collect();
            if let Ok(mut map) = cb.targets.lock() {
                map.insert(handle.as_str().to_owned(), words.clone());
            }
            if let Some(link) = cb.link(handle) {
                link.save(&words, cb.clock.now());
            }
            Ok(words)
        }
        _ => Err(DriverError::Failed {
            summary: "unreadable box provider output for ssh target".to_owned(),
        }),
    }
}

impl Crabbox {
    /// Keeps whether `crabbox ssh` takes `--no-probe` in `known`, so every driver sharing it asks
    /// crabbox once.
    pub(crate) fn sharing_no_probe(mut self, known: Arc<OnceLock<bool>>) -> Self {
        self.no_probe = known;
        self
    }

    /// Whether `crabbox ssh` takes `--no-probe`, which skips the ssh probes it runs before
    /// printing the words; asked once. A crabbox that does not say so, or cannot be asked, is
    /// looked up without the flag.
    fn ssh_skips_probe(&self) -> bool {
        *self.no_probe.get_or_init(|| {
            let spec = Self::spec(LOOKUP_DEADLINE).args(["ssh", "--help"]);
            self.proc.run(&spec).is_ok_and(|out| {
                out.stderr.contains("-no-probe") || out.stdout.contains("-no-probe")
            })
        })
    }
}

/// Splits one line of single-quoted shell words; `None` when a quote is left open.
fn split_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '\'') => quoted = false,
            (true, _) => cur.push(c),
            (false, '\'') => {
                quoted = true;
                started = true;
            }
            (false, '\\') => {
                cur.push(chars.next()?);
                started = true;
            }
            (false, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (false, _) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if started {
        words.push(cur);
    }
    Some(words)
}

#[cfg(test)]
mod tests;
