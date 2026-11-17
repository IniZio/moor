#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use super::args::{agent_run, work_memory_mib, work_run};
use super::names::Names;
use super::{net, Images, Settings};
use crate::boxes::Slug;

const SLUG: &str = "moor-3b3b3b3b-9001";

fn slug() -> Slug {
    Slug::parse(SLUG).unwrap()
}

fn settings() -> Settings {
    Settings::new(
        Images {
            agent: Some("agent:dev".to_owned()),
            work: Some("work:dev".to_owned()),
        },
        "net",
    )
}

#[test]
fn every_object_is_named_from_the_slug() {
    let n = Names::of(&slug());
    assert_eq!(n.agent, "moor-3b3b3b3b-9001-agent");
    assert_eq!(n.work, "moor-3b3b3b3b-9001-work");
    assert!(n.volumes().iter().all(|v| v.starts_with(SLUG)));
    let mut all = n.volumes().to_vec();
    all.extend([n.agent.as_str(), n.work.as_str()]);
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 7, "{all:?}");
}

#[test]
fn the_release_images_are_the_default() {
    let s = Settings::new(Images::default(), "moor");
    assert_eq!(
        s.agent_image,
        format!("ghcr.io/inizio/moor-agent:{}", crate::VERSION)
    );
    assert_eq!(
        s.work_image,
        format!("ghcr.io/inizio/moor-work:{}", crate::VERSION)
    );
}

#[test]
fn the_agent_place_is_unprivileged_and_never_restarted_by_docker() {
    let a = agent_run(&settings(), &slug(), &Names::of(&slug())).join(" ");
    assert!(a.starts_with("run -d --name moor-3b3b3b3b-9001-agent --hostname moor-3b3b3b3b-9001"));
    assert!(a.contains("--network net --restart no"));
    assert!(a.contains("--add-host moor-host:host-gateway"));
    assert!(a.contains("-v moor-3b3b3b3b-9001-tree:/home/agent/moor"));
    assert!(!a.contains("--privileged"));
    assert!(a.ends_with(" agent:dev"));
}

#[test]
fn the_work_place_joins_the_agent_places_network_and_is_privileged() {
    let n = Names::of(&slug());
    let w = work_run(&settings(), &slug(), &n, 4096).join(" ");
    assert!(w.contains("--network container:moor-3b3b3b3b-9001-agent"));
    assert!(w.contains("--privileged --restart no"));
    assert!(w.contains("--memory 4096m --memory-swap 4096m"));
    assert!(w.contains("-v moor-3b3b3b3b-9001-docker:/var/lib/docker"));
    assert!(!w.contains("--hostname"));
    assert!(w.ends_with(" work:dev"));
}

#[test]
fn a_class_sets_the_work_places_memory() {
    assert_eq!(work_memory_mib(Some("small")), 4096);
    assert_eq!(work_memory_mib(Some("xlarge")), 32_768);
    assert_eq!(work_memory_mib(None), 8192);
    assert_eq!(work_memory_mib(Some("nonsense")), 8192);
}

#[test]
fn the_route_device_is_read_from_ip_route_get() {
    let line = "140.82.112.6 dev tailscale0 table 52 src 100.64.0.2 uid 1000 \n    cache";
    assert_eq!(net::device_of(line), Some("tailscale0"));
    assert_eq!(
        net::device_of("RTNETLINK answers: Network is unreachable"),
        None
    );
}

#[test]
fn docker_ps_lines_become_each_boxes_two_places() {
    use super::parse_ps;
    use crate::boxes::provider::PlaceState::{Down, Up};
    let listed = "moor-3b3b3b3b-9001 agent running\n\
                  moor-3b3b3b3b-9001 work exited\n\
                  moor-3b3b3b3b-9002 agent running\n\
                  stranger agent running\n";
    let boxes = parse_ps(listed);
    assert_eq!(boxes.len(), 2, "{boxes:?}");
    assert_eq!(boxes[&slug()], (Up, Down));
    let other = Slug::parse("moor-3b3b3b3b-9002").unwrap();
    assert_eq!(boxes[&other], (Up, Down));
}
