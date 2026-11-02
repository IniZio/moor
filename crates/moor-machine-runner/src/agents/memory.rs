//! A machine agent's memory report as the controller records it.

use moor_client::link::Report;
use moor_core::box_key::parse_pool_slug;
use moor_core::request::{BoxHostMemory, MemoryReport};

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

/// What `report` says its machine's Boxes hold; a slug that is not a pool lease's is dropped.
pub(super) fn box_memory(machine_id: i64, report: &Report) -> Vec<BoxHostMemory> {
    report
        .boxes
        .iter()
        .filter_map(|b| {
            let (controller, box_id) = parse_pool_slug(&b.slug)?;
            Some(BoxHostMemory {
                machine_id,
                controller,
                box_id,
                host_memory_mib: b.host_memory_mib,
            })
        })
        .collect()
}
