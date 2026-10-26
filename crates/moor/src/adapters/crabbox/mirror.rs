//! Mirrors a directory of a Box into a host directory with rsync over the Box's shared ssh
//! connection.
//!
//! rsync runs on the host and starts the Box's rsync through the same ssh words every other
//! short call uses, so the copy rides the Box's master and keeps crabbox's host-key pinning.
//! Links stay links, modes are kept, files gone from the Box go from the host, and `.git` is
//! left out. A later mirror sends only what changed.

use std::path::Path;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{DriverError, MirrorStats};
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::peek::ssh_target;
use super::{link, touch, Crabbox};

/// Time allowed for one mirror.
const MIRROR_DEADLINE: Duration = Duration::from_mins(10);
/// What the Box prints on stderr when it has no rsync.
const NO_RSYNC: &str = "moor-box-has-no-rsync";

/// Mirrors the Box directory `from` into the host directory `to`.
pub(super) fn mirror_out(
    cb: &Crabbox,
    handle: &BoxHandle,
    from: &BoxPath,
    to: &Path,
) -> Result<MirrorStats, DriverError> {
    std::fs::create_dir_all(to).map_err(|e| {
        failed(&format!("cannot make the destination ({})", e.kind()))
    })?;
    let _ = touch::heartbeat(cb, handle);
    let mut words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    let host = words
        .pop()
        .ok_or_else(|| failed("the Box has no ssh destination"))?;
    let ssh = cb
        .proc
        .program_path(Tool::Ssh)
        .ok_or_else(|| failed("the ssh program is unknown"))?;
    let mut rsh = vec![rsh_word(&ssh.to_string_lossy())?];
    for w in &words {
        rsh.push(rsh_word(w)?);
    }
    let source = format!("{}:{}/", remote_host(&host), from.as_str().trim_end_matches('/'));
    let spec = ProcSpec::new(Tool::Rsync, Deadline(MIRROR_DEADLINE))
        .args(["-a", "--delete", "--exclude", ".git", "--stats", "-e"])
        .arg(rsh.join(" "))
        .arg(format!(
            "--rsync-path=command -v rsync >/dev/null 2>&1 || {{ echo {NO_RSYNC} >&2; exit 1; }}; rsync"
        ))
        .arg("--")
        .arg(source)
        .arg(format!("{}/", to.display()));
    let ran = cb.proc.run(&spec);
    if let Err(e) = &ran {
        link::failed(cb, handle, e);
    }
    match ran {
        Ok(out) => stats(&out.stdout)
            .ok_or_else(|| failed("rsync printed no stats")),
        Err(ProcError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(DriverError::Unsupported("rsync on the host"))
        }
        Err(ProcError::NonZero { stderr_tail, .. }) if stderr_tail.contains(NO_RSYNC) => {
            Err(DriverError::Unsupported("rsync on the Box"))
        }
        Err(ProcError::Timeout { after, .. }) => Err(DriverError::Timeout(after)),
        Err(ProcError::Spawn(_)) => Err(failed("rsync could not be started")),
        Err(e) => Err(link::lost(cb, handle, &e).unwrap_or_else(|| match e {
            ProcError::NonZero {
                code, stderr_tail, ..
            } => failed(&format!(
                "rsync exited {code}: {}",
                stderr_tail.lines().last().unwrap_or_default().trim()
            )),
            _ => failed("rsync broke off"),
        })),
    }
}

/// `word` quoted for rsync's `-e`, which splits on spaces and honours quotes but not
/// backslashes.
fn rsh_word(word: &str) -> Result<String, DriverError> {
    if !word.contains('\'') {
        Ok(format!("'{word}'"))
    } else if !word.contains('"') {
        Ok(format!("\"{word}\""))
    } else {
        Err(failed("an ssh word holds both quote characters"))
    }
}

/// The ssh destination as rsync reads a host: an IPv6 address goes in brackets.
fn remote_host(host: &str) -> String {
    let (user, addr) = host
        .rsplit_once('@')
        .map_or(("", host), |(u, a)| (u, a));
    let addr = if addr.contains(':') && !addr.starts_with('[') {
        format!("[{addr}]")
    } else {
        addr.to_owned()
    };
    if user.is_empty() {
        addr
    } else {
        format!("{user}@{addr}")
    }
}

/// The files and bytes in rsync's `--stats` report.
fn stats(out: &str) -> Option<MirrorStats> {
    let field = |label: &str| {
        out.lines()
            .find_map(|l| l.trim().strip_prefix(label))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.replace(',', "").parse().ok())
    };
    Some(MirrorStats {
        files: field("Number of regular files transferred:")?,
        bytes: field("Total bytes received:")?,
    })
}

/// A mirror failure saying `why`.
fn failed(why: &str) -> DriverError {
    DriverError::Failed {
        summary: format!("box mirror failed: {why}"),
    }
}

#[cfg(test)]
mod tests;
