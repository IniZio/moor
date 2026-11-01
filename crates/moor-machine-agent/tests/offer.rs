//! What `pool::offer` reports from a pool's flags and its crabbox.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::path::Path;

use moor_machine_agent::pool::{self, Pool};
use moor_machine_agent::Machine;

#[allow(dead_code, reason = "shared with the other pool tests")]
mod fake_crabbox;

/// A machine whose pool uses the crabbox at `crabbox`.
fn machine(crabbox: &Path) -> Machine {
    let mut machine = Machine::from_env();
    machine.pool = Some(Pool {
        provider: "fake".to_owned(),
        max: Some(3),
        reserve_mib: None,
        cache_gb: Some(50),
        crabbox: crabbox.to_owned(),
    });
    machine
}

#[tokio::test]
async fn features_come_from_the_describe_json_sorted() {
    let home = tempfile::tempdir().unwrap();
    let crabbox = fake_crabbox::install(home.path());
    let describe = r#"{"name":"fake","capabilities":{"features":["snapshots","cache-volume"]}}"#;
    std::fs::write(home.path().join("describe.json"), describe).unwrap();
    let offer = pool::offer(&machine(&crabbox)).await.unwrap();
    assert_eq!(offer.features, ["cache-volume", "snapshots"]);
    assert_eq!((offer.max_boxes, offer.cache_gb), (Some(3), Some(50)));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_offer_reports_host_memory_cpus_and_the_reserve() {
    let home = tempfile::tempdir().unwrap();
    let crabbox = fake_crabbox::install(home.path());
    let mut m = machine(&crabbox);
    if let Some(p) = m.pool.as_mut() {
        p.reserve_mib = Some(2048);
    }
    let offer = pool::offer(&m).await.unwrap();
    assert!(offer.mem_mib.is_some_and(|m| m > 0), "{offer:?}");
    assert!(offer.cpus.is_some_and(|c| c > 0), "{offer:?}");
    assert_eq!(offer.reserve_mib, Some(2048));
}

/// A crabbox that records its arguments beside itself, and fails as an older one does when
/// `reject` is set.
fn recording_crabbox(dir: &Path, reject: bool) -> std::path::PathBuf {
    let path = dir.join("crabbox");
    let script = format!(
        "#!/bin/sh\necho \"$*\" > \"$(dirname \"$0\")/args\"\n{}echo '{{\"capabilities\":{{\"features\":[\"cache-volume\"]}}}}'\n",
        if reject {
            "echo 'flag provided but not defined: -configured' >&2\nexit 2\n"
        } else {
            ""
        }
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn features_are_asked_for_with_the_configured_flag() {
    let home = tempfile::tempdir().unwrap();
    let crabbox = recording_crabbox(home.path(), false);
    let offer = pool::offer(&machine(&crabbox)).await.unwrap();
    assert_eq!(offer.features, ["cache-volume"]);
    let args = std::fs::read_to_string(home.path().join("args")).unwrap();
    assert_eq!(args.trim(), "providers describe fake --configured --json");
}

#[tokio::test]
async fn a_crabbox_that_rejects_the_configured_flag_leaves_features_empty() {
    let home = tempfile::tempdir().unwrap();
    let crabbox = recording_crabbox(home.path(), true);
    let offer = pool::offer(&machine(&crabbox)).await.unwrap();
    assert!(offer.features.is_empty());
    assert_eq!((offer.max_boxes, offer.cache_gb), (Some(3), Some(50)));
}

#[tokio::test]
async fn a_describe_that_is_not_json_or_lacks_features_leaves_them_empty() {
    let home = tempfile::tempdir().unwrap();
    let crabbox = fake_crabbox::install(home.path());
    for text in ["not json", "{}", r#"{"capabilities":{"features":"x"}}"#, ""] {
        std::fs::write(home.path().join("describe.json"), text).unwrap();
        let offer = pool::offer(&machine(&crabbox)).await.unwrap();
        assert!(offer.features.is_empty(), "{text}");
        assert_eq!((offer.max_boxes, offer.cache_gb), (Some(3), Some(50)));
    }
}

#[tokio::test]
async fn a_crabbox_that_fails_or_is_missing_leaves_features_empty() {
    let home = tempfile::tempdir().unwrap();
    let failing = home.path().join("failing");
    std::fs::write(
        &failing,
        "#!/bin/sh\necho '{\"capabilities\":{\"features\":[\"a\"]}}'\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &failing,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    for crabbox in [failing, home.path().join("absent")] {
        let offer = pool::offer(&machine(&crabbox)).await.unwrap();
        assert!(offer.features.is_empty(), "{}", crabbox.display());
        assert_eq!((offer.max_boxes, offer.cache_gb), (Some(3), Some(50)));
    }
}

#[tokio::test]
async fn a_machine_without_a_pool_offers_nothing() {
    assert_eq!(pool::offer(&Machine::from_env()).await, None);
}
