//! Why a Box on a personal node starts without cache volumes.

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
