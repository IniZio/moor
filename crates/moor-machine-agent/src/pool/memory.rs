//! The host's memory: its total for the offer, and the live reports the agent sends.

use std::path::Path;

use moor_client::link::ops;
use moor_client::link::{self, Opened, Report};

use super::Pool;

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
fn report(proc: &Path, home: Option<&Path>) -> Option<Report> {
    let info = std::fs::read_to_string(proc.join("meminfo")).ok()?;
    let psi = std::fs::read_to_string(proc.join("pressure/memory")).ok();
    let disk = home.and_then(disk_mib);
    Some(Report {
        mem_available_mib: meminfo_mib(&info, "MemAvailable:")?,
        mem_total_mib: meminfo_mib(&info, "MemTotal:")?,
        psi_some_avg10: psi.as_deref().and_then(|p| avg10(p, "some ")),
        psi_full_avg10: psi.as_deref().and_then(|p| avg10(p, "full ")),
        disk_free_mib: disk.map(|d| d.0),
        disk_total_mib: disk.map(|d| d.1),
        boxes: Vec::new(),
    })
}

/// Free and total MiB of the filesystem holding `home`; none when it cannot be read.
pub(super) fn disk_mib(home: &Path) -> Option<(u64, u64)> {
    let fs = rustix::fs::statvfs(home).ok()?;
    let mib = |blocks: u64| blocks.saturating_mul(fs.f_frsize) / (1024 * 1024);
    Some((mib(fs.f_bavail), mib(fs.f_blocks)))
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

/// Sends a [`report`] of `proc` on `stream` now and every [`link::REPORT_EVERY`] until it closes,
/// with the memory `pool`'s leases hold when its provider says.
pub(crate) async fn send_reports<W>(
    mut stream: W,
    proc: &Path,
    home: Option<&Path>,
    pool: Option<&Pool>,
) where
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    let mut tick = tokio::time::interval(link::REPORT_EVERY);
    let mut first = true;
    loop {
        tick.tick().await;
        let Some(mut report) = self::report(proc, home) else {
            continue;
        };
        // Linking runs no `crabbox` call: the first report carries no Box memory.
        if let (false, Some(home), Some(pool)) = (first, home, pool) {
            report.boxes = super::box_memory(home, pool).await.unwrap_or_default();
        }
        first = false;
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
