//! A machine agent's memory report as the controller records it.

use moor_client::link::Report;
use moor_core::request::MemoryReport;

/// `report`, received now.
pub(super) fn received(report: &Report) -> MemoryReport {
    MemoryReport {
        at: crate::claim::now(),
        available_mib: report.mem_available_mib,
        total_mib: report.mem_total_mib,
        psi_centi: report.psi_some_avg10.map(centi),
        psi_full_centi: report.psi_full_avg10.map(centi),
        disk_free_mib: report.disk_free_mib,
        disk_total_mib: report.disk_total_mib,
    }
}

/// A PSI percentage in hundredths.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "PSI is a percentage, clamped to 0..=100"
)]
fn centi(psi: f64) -> u32 {
    (psi.clamp(0.0, 100.0) * 100.0).round() as u32
}
