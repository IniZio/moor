//! Pausing a lease: its port tunnels stop first, and stay refused until the lease is up again.

use std::io::Write as _;

use moor_client::link::lease::Answer;

use super::{port, Crabbox, CALL, TAIL};

/// Pauses the lease with `env` added to crabbox's.
pub(super) async fn pause(lease: &Crabbox<'_>, env: &[(&str, String)]) -> Result<Answer, String> {
    let slug = lease.slug.unwrap_or_default();
    port::stop_lease(slug).await;
    let paused = match lease.run_both("pause", &["--id"], CALL, &[0], env).await {
        Ok((_, said)) => {
            log_pause(slug, &said);
            Ok(Answer::Done)
        }
        Err(e) if e.contains("does not support pause") => Ok(Answer::Unsupported),
        Err(e) => Err(e),
    };
    if !matches!(paused, Ok(Answer::Done)) {
        port::allow(slug).await;
    }
    paused
}

/// Logs what crabbox `said` while pausing `slug`: only there can a host see whether the caches
/// were saved. It is another program's output, so only its end is kept and no control character
/// reaches the log.
fn log_pause(slug: &str, said: &str) {
    let mut cut = said.len().saturating_sub(TAIL);
    while !said.is_char_boundary(cut) {
        cut += 1;
    }
    let mut out = std::io::stderr().lock();
    for line in said.get(cut..).unwrap_or_default().lines() {
        let line: String = line
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let line = line.trim();
        if !line.is_empty() {
            let _ = writeln!(out, "moor-machine-agent: pause {slug}: {line}");
        }
    }
}
