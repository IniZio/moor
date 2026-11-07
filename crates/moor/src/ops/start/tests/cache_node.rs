//! Why a Box on a personal node starts without cache volumes.

use moor_core::domain::cache_save::{CacheSave, CacheSaveDenied};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;

use super::placement::{cached_world, planned_volumes};
use super::World;
use crate::ops::nodes;
use crate::testing::cx::REQUESTER_USER;

fn cold_reasons(w: &World) -> Vec<String> {
    let store = w.cx.store().unwrap();
    let id = store
        .open_box_by_name(&crate::testing::box_name("demo"))
        .unwrap()
        .unwrap()
        .id;
    super::warm::events(w, id, "cache_cold")
        .iter()
        .map(|p| p["reason"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_personal_node_without_cache_volumes_records_why_the_box_is_cold() {
    for features in [&["other"][..], &[]] {
        let w = cached_world(features, true);
        planned_volumes(&w);
        assert_eq!(cold_reasons(&w), ["machine_lacks_feature"], "{features:?}");
    }
}

#[test]
fn a_personal_node_whose_link_drops_before_the_plan_records_why_the_box_is_cold() {
    let w = cached_world(&["cache-volume"], true);
    planned_volumes(&w);
    let store = w.cx.store().unwrap();
    let row = store
        .open_box_by_name(&crate::testing::box_name("demo"))
        .unwrap()
        .unwrap();
    let links = store.user_machine_links(REQUESTER_USER).unwrap();
    let machine_id = links.first().unwrap().1.machine_id;
    let link = MachineLink {
        machine_id,
        online: false,
        since: Ts::from_millis(2),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: vec!["cache-volume".to_owned()],
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![link])).unwrap();
    let cfg =
        crate::ops::common::facts::settings_of(&w.cx, &row, &moor_core::config::Flags::default())
            .unwrap();
    let provider = row.boxed.clone().unwrap().provider;
    let caps = w.cx.driver().caps(&provider).unwrap();
    crate::ops::common::cache::for_start(&w.cx, store, &row, &cfg, &provider, caps);
    assert_eq!(cold_reasons(&w), ["machine_offline"]);
}

#[test]
fn a_personal_node_that_mounts_cache_volumes_records_nothing() {
    let w = cached_world(&["cache-volume"], true);
    planned_volumes(&w);
    assert!(cold_reasons(&w).is_empty());
}

fn saves(w: &World) -> bool {
    let store = w.cx.store().unwrap();
    let row = store
        .open_box_by_name(&crate::testing::box_name("demo"))
        .unwrap()
        .unwrap();
    crate::ops::common::cache::saves_on_pause(&w.cx, store, &row).is_ok()
}

#[test]
fn a_personal_node_saves_at_a_pause_only_when_its_online_agent_reports_it() {
    let w = cached_world(&["cache-volume", "cache-save-on-pause"], true);
    planned_volumes(&w);
    assert!(saves(&w));
    let w = cached_world(&["cache-volume"], true);
    planned_volumes(&w);
    assert!(!saves(&w));
    let store_links = |w: &World| w.cx.store().unwrap().user_machine_links(REQUESTER_USER);
    let w = cached_world(&["cache-volume", "cache-save-on-pause"], true);
    planned_volumes(&w);
    let machine_id = store_links(&w).unwrap().first().unwrap().1.machine_id;
    let offline = MachineLink {
        machine_id,
        online: false,
        since: Ts::from_millis(2),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: vec!["cache-save-on-pause".to_owned()],
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![offline])).unwrap();
    assert!(!saves(&w));
}

/// What a sleep of the world's Box would decide, with its agent settled or not.
fn decided(w: &World, settled: Result<(), CacheSaveDenied>) -> Option<CacheSave> {
    let store = w.cx.store().unwrap();
    let row = store
        .open_box_by_name(&crate::testing::box_name("demo"))
        .unwrap()
        .unwrap();
    crate::ops::common::cache::save_decision(&w.cx, store, &row, settled)
}

const fn denied(reason: CacheSaveDenied) -> CacheSave {
    CacheSave::Denied { reason }
}

#[test]
fn a_node_that_reports_the_feature_allows_a_settled_sleep_and_passes_on_why_not() {
    let w = cached_world(&["cache-volume", "cache-save-on-pause"], true);
    planned_volumes(&w);
    assert_eq!(decided(&w, Ok(())), Some(CacheSave::Allowed));
    let reason = CacheSaveDenied::RunInFlight;
    assert_eq!(decided(&w, Err(reason)), Some(denied(reason)));
}

#[test]
fn a_box_with_volumes_on_a_node_without_the_feature_is_denied_for_that() {
    let w = cached_world(&["cache-volume"], true);
    assert!(planned_volumes(&w) > Some(0));
    assert_eq!(
        decided(&w, Ok(())),
        Some(denied(CacheSaveDenied::NoFeature))
    );
}

#[test]
fn a_missing_feature_outranks_an_unsettled_agent() {
    let w = cached_world(&["cache-volume"], true);
    planned_volumes(&w);
    assert_eq!(
        decided(&w, Err(CacheSaveDenied::AgentUnread)),
        Some(denied(CacheSaveDenied::NoFeature))
    );
}

#[test]
fn a_box_whose_node_went_offline_is_denied_for_that() {
    let w = cached_world(&["cache-volume", "cache-save-on-pause"], true);
    planned_volumes(&w);
    let store = w.cx.store().unwrap();
    let mut link = store
        .user_machine_links(REQUESTER_USER)
        .unwrap()
        .remove(0)
        .1;
    link.online = false;
    nodes::set_links(&w.cx, &crate::testing::one_runner(vec![link])).unwrap();
    assert_eq!(
        decided(&w, Ok(())),
        Some(denied(CacheSaveDenied::MachineOffline))
    );
}
