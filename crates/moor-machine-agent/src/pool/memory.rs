//! The host's memory: its total for the offer, and the live reports the agent sends.

use std::path::Path;

use moor_client::link::ops;
use moor_client::link::{self, Opened, Report};

/// The host's total memory from `/proc/meminfo`; none off Linux.
pub(super) fn host_mem_mib() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    mem_total_mib(&info)
}

/// `MemTotal` of a `/proc/meminfo` text, in MiB.
fn mem_total_mib(info: &str) -> Option<u64> {
    meminfo_mib(info, "MemTotal:")
}

/// The host's memory now from the `/proc` at `proc`; none without a readable `meminfo`.
fn report(proc: &Path) -> Option<Report> {
    let info = std::fs::read_to_string(proc.join("meminfo")).ok()?;
    let psi = std::fs::read_to_string(proc.join("pressure/memory")).ok();
    Some(Report {
        mem_available_mib: meminfo_mib(&info, "MemAvailable:")?,
        mem_total_mib: meminfo_mib(&info, "MemTotal:")?,
        psi_some_avg10: psi.as_deref().and_then(|p| avg10(p, "some ")),
        psi_full_avg10: psi.as_deref().and_then(|p| avg10(p, "full ")),
    })
}

/// The `avg10` of the `line` line (`some ` or `full `) of a PSI file.
fn avg10(psi: &str, line: &str) -> Option<f64> {
    psi.lines()
        .find_map(|l| l.strip_prefix(line))?
        .split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))?
        .parse()
        .ok()
}

/// Sends a [`report`] of `proc` on `stream` now and every [`link::REPORT_EVERY`] until it closes.
pub(crate) async fn send_reports<W>(mut stream: W, proc: &Path)
where
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    let mut tick = tokio::time::interval(link::REPORT_EVERY);
    loop {
        tick.tick().await;
        let Some(report) = self::report(proc) else {
            continue;
        };
        if ops::write_line(&mut stream, &Opened::Report { report })
            .await
            .is_err()
        {
            return;
        }
    }
}

/// The `key` line of a `/proc/meminfo` text, in MiB.
fn meminfo_mib(info: &str, key: &str) -> Option<u64> {
    let kib: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix(key))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kib / 1024)
}

#[cfg(test)]
mod tests;
