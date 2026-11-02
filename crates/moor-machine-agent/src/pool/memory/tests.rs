//! Tests for the agent's memory reports.

use std::path::Path;

use moor_client::link::ops::read_line;
use moor_client::link::{Opened, Report};

use super::{report, send_reports};

/// A `/proc` under `dir` with `meminfo` and, when given, `pressure/memory`.
fn fake_proc(dir: &Path, psi: Option<&str>) {
    std::fs::write(
        dir.join("meminfo"),
        "MemTotal:       32768000 kB\nMemFree:          100000 kB\nMemAvailable:    8192000 kB\n",
    )
    .unwrap();
    if let Some(psi) = psi {
        std::fs::create_dir_all(dir.join("pressure")).unwrap();
        std::fs::write(dir.join("pressure/memory"), psi).unwrap();
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moor-report-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_report_reads_available_total_and_both_psi_avg10s() {
    let dir = scratch("psi");
    fake_proc(
        &dir,
        Some("some avg10=12.50 avg60=3.00 avg300=1.00 total=99\nfull avg10=1.00 avg60=0.00 avg300=0.00 total=9\n"),
    );
    assert_eq!(
        report(&dir, None),
        Some(Report {
            mem_available_mib: 8000,
            mem_total_mib: 32000,
            psi_some_avg10: Some(12.5),
            psi_full_avg10: Some(1.0),
            disk_free_mib: None,
            disk_total_mib: None,
            boxes: Vec::new(),
        })
    );
}

#[test]
fn a_host_without_psi_reports_none() {
    let dir = scratch("nopsi");
    fake_proc(&dir, None);
    assert_eq!(report(&dir, None).and_then(|r| r.psi_some_avg10), None);
}

#[tokio::test]
async fn the_agent_sends_a_report_at_once_on_its_stream() {
    let dir = scratch("send");
    fake_proc(
        &dir,
        Some("some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"),
    );
    let (near, mut far) = tokio::io::duplex(4096);
    let sending = tokio::spawn({
        let dir = dir.clone();
        async move { send_reports(near, &dir, None, None).await }
    });
    let first: Opened = read_line(&mut far).await.unwrap();
    assert_eq!(
        first,
        Opened::Report {
            report: Report {
                mem_available_mib: 8000,
                mem_total_mib: 32000,
                psi_some_avg10: Some(0.0),
                psi_full_avg10: None,
                disk_free_mib: None,
                disk_total_mib: None,
                boxes: Vec::new(),
            }
        }
    );
    drop(far);
    sending.abort();
}

#[test]
fn a_report_carries_the_free_and_total_disk_of_the_home_filesystem() {
    let dir = scratch("disk");
    fake_proc(&dir, None);
    let r = report(&dir, Some(&dir)).unwrap();
    let (free, total) = (r.disk_free_mib.unwrap(), r.disk_total_mib.unwrap());
    assert!(total > 0 && free <= total, "{free} of {total}");
}

#[test]
fn a_home_that_does_not_exist_reports_no_disk() {
    let dir = scratch("nodisk");
    fake_proc(&dir, None);
    let r = report(&dir, Some(&dir.join("missing"))).unwrap();
    assert_eq!((r.disk_free_mib, r.disk_total_mib), (None, None));
}
