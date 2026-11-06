//! Tests for reading `crabbox list --json` rows.

use moor_client::link::BoxMemory;
use serde_json::json;

use super::{held_memory, leases};

#[test]
fn a_row_carries_what_it_needs_to_resume_and_a_bad_number_counts_as_absent() {
    let rows = [
        json!({"slug": "moor-0badcafe-1", "state": "paused", "hostMemoryMiB": 0, "resumeMemoryMiB": 6144}),
        json!({"slug": "moor-0badcafe-2", "state": "paused", "resumeMemoryMiB": 2048}),
        json!({"slug": "moor-0badcafe-3", "state": "paused", "hostMemoryMiB": 0, "resumeMemoryMiB": "lots"}),
    ];
    let got: Vec<(String, u64, Option<u64>)> = held_memory(&rows)
        .into_iter()
        .map(|b| (b.slug, b.host_memory_mib, b.resume_memory_mib))
        .collect();
    assert_eq!(
        got,
        [
            ("moor-0badcafe-1".to_owned(), 0, Some(6144)),
            ("moor-0badcafe-2".to_owned(), 0, Some(2048)),
            ("moor-0badcafe-3".to_owned(), 0, None),
        ]
    );
}

#[test]
fn a_row_with_host_memory_reports_it_and_one_without_is_left_out() {
    let rows = [
        json!({"slug": "moor-0badcafe-1", "state": "running", "hostMemoryMiB": 2048}),
        json!({"slug": "moor-0badcafe-2", "state": "paused", "hostMemoryMiB": 0}),
        json!({"slug": "moor-0badcafe-3", "state": "running"}),
        json!({"slug": "other-4", "hostMemoryMiB": 7}),
    ];
    assert_eq!(
        held_memory(&rows),
        [
            BoxMemory {
                slug: "moor-0badcafe-1".to_owned(),
                host_memory_mib: 2048,
                resume_memory_mib: None,
            },
            BoxMemory {
                slug: "moor-0badcafe-2".to_owned(),
                host_memory_mib: 0,
                resume_memory_mib: None,
            },
        ]
    );
}

#[test]
fn a_row_with_fields_this_build_does_not_know_still_lists_and_reports() {
    let rows = [json!({
        "labels": {"slug": "moor-0badcafe-5"},
        "status": "running",
        "hostMemoryMiB": 512,
        "futureField": {"deep": [1, 2]},
    })];
    assert_eq!(
        leases(&rows).first().map(|l| l.state.as_str()),
        Some("running")
    );
    assert_eq!(
        held_memory(&rows).first().map(|b| b.host_memory_mib),
        Some(512)
    );
}

#[test]
fn a_host_memory_that_is_not_a_count_reads_as_unknown() {
    let rows = [json!({"slug": "moor-0badcafe-6", "hostMemoryMiB": "lots"})];
    assert!(held_memory(&rows).is_empty());
    assert_eq!(leases(&rows).len(), 1);
}
