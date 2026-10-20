//! Reads one file in a Box over ssh, never through `crabbox run`.
//!
//! `crabbox run` holds the Box's workspace for the whole command, so a read
//! issued through it would wait for a running command. `crabbox ssh` only
//! prints the ssh command line for the Box, so the read goes straight to ssh.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::errors::{map_error, Op};
use super::{link, split_handle, static_env, target_args, Crabbox};

/// Time allowed for the read.
const READ_DEADLINE: Duration = Duration::from_secs(5);
/// Remote exit code that means the file does not exist.
const MISSING: i32 = 44;

/// Reads `path` in the Box behind `handle`; `None` when the file is absent.
/// The target lookup runs in `host_cwd`, so the lease claim matches that repo.
pub(super) fn peek_file(
    cb: &Crabbox,
    handle: &BoxHandle,
    path: &BoxPath,
    host_cwd: &Path,
) -> Result<Option<String>, DriverError> {
    let words = link::args(cb, handle, ssh_target(cb, handle, host_cwd)?);
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
        Err(e) => Err(link::lost(&e).unwrap_or_else(|| DriverError::Failed {
            summary: "box file read failed".to_owned(),
        })),
    }
}

/// The ssh arguments that reach the Box behind `handle`, without the leading `ssh`.
/// The lookup runs in `host_cwd`, so the lease claim matches that repo.
pub(super) fn ssh_target(
    cb: &Crabbox,
    handle: &BoxHandle,
    host_cwd: &Path,
) -> Result<Vec<String>, DriverError> {
    // `crabbox ssh` touches the lease, so a lookup also resets crabbox's idle timer;
    // lookups are reused within one command and, for a while, through the Box's link, and
    // idle Boxes are slept by moor's `sleep_after`, not by that timer.
    let key = (handle.as_str().to_owned(), host_cwd.to_path_buf());
    if let Some(words) = cb.targets.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(words);
    }
    let shared = cb.link(handle);
    if let Some(words) = shared.as_ref().and_then(|l| l.saved(cb.clock.now())) {
        if let Ok(mut map) = cb.targets.lock() {
            map.insert(key, words.clone());
        }
        return Ok(words);
    }
    // Secret-token providers need --show-secret, not yet passed.
    let (provider, _) = split_handle(handle);
    let lookup = static_env(
        Crabbox::spec(cb.lookup_deadline(provider.unwrap_or_default()))
            .arg("ssh")
            .args(target_args(handle))
            .cwd(host_cwd),
        handle,
    );
    // Another crabbox call on the same lease can rewrite its claim mid-lookup.
    let line = cb
        .retry_claim_race(|| cb.proc.run(&lookup))
        .map_err(|e| map_error(Op::Target, split_handle(handle).0, &e))?
        .stdout;
    match split_words(&line) {
        Some(words) if words.first().is_some_and(|w| w == "ssh") => {
            let words: Vec<String> = words.into_iter().skip(1).collect();
            if let Ok(mut map) = cb.targets.lock() {
                map.insert(key, words.clone());
            }
            if let Some(shared) = shared {
                shared.save(&words, cb.clock.now());
            }
            Ok(words)
        }
        _ => Err(DriverError::Failed {
            summary: "unreadable box provider output for ssh target".to_owned(),
        }),
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
