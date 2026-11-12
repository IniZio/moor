//! Tests for the Boxes slice unit.

use super::{install, unit, SLICE};
use crate::service::Manager;

#[test]
fn the_unit_holds_boxes_to_the_host_less_the_reserve_and_leaves_headroom_above_it() {
    let text = unit(32_000, 7_296);
    assert!(text.contains("\n[Slice]\n"), "{text}");
    // MemoryHigh is total less the reserve; MemoryMax is total less half the reserve.
    assert!(text.contains("\nMemoryHigh=24704M\n"), "{text}");
    assert!(text.contains("\nMemoryMax=28352M\n"), "{text}");
    assert!(text.contains("\nCPUWeight=50\n"), "{text}");
}

#[test]
fn a_reserve_past_the_host_leaves_the_slice_a_floor_rather_than_zero() {
    let text = unit(2_048, 4_096);
    assert!(text.contains("\nMemoryHigh=1M\n"), "{text}");
    assert!(text.contains("\nMemoryMax=1M\n"), "{text}");
}

#[test]
fn install_writes_the_unit_once_and_rewrites_it_for_a_new_reserve() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::Systemd {
        dir: dir.path().to_path_buf(),
    };
    let file = dir.path().join(SLICE);
    assert!(install(&manager, 32_000, 7_296).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), unit(32_000, 7_296));
    assert!(!install(&manager, 32_000, 7_296).unwrap(), "unchanged");
    assert!(install(&manager, 32_000, 4_096).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), unit(32_000, 4_096));
}

#[test]
fn a_launchd_host_gets_no_slice() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::Launchd {
        dir: dir.path().to_path_buf(),
        uid: 501,
    };
    assert!(!install(&manager, 32_000, 7_296).unwrap());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_launchd_host_tells_the_provider_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::Launchd {
        dir: dir.path().to_path_buf(),
        uid: 501,
    };
    let config = dir.path().join("config.yaml");
    let mut said = Vec::new();
    super::tell_provider(&manager, &config, "external", &mut |l| {
        said.push(l.to_owned());
    });
    assert!(!config.exists());
    assert!(said.is_empty(), "{said:?}");
}
