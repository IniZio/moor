#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use super::args::{agent_run, gid_of, work_memory_mib, work_run};
use super::cli::parse_ended;
use super::names::Names;
use super::{net, Images, Settings};
use crate::boxes::provider::Ended;
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
    assert_eq!(all.len(), 10, "{all:?}");
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
    let a = agent_run(&settings(), &slug(), &Names::of(&slug()), &[]).join(" ");
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
    let w = work_run(&settings(), &slug(), &n, 4096, None, &[]).join(" ");
    assert!(w.contains("--network container:moor-3b3b3b3b-9001-agent"));
    assert!(w.contains("--privileged --restart no"));
    assert!(w.contains("--memory 4096m --memory-swap 4096m"));
    assert!(w.contains("-v moor-3b3b3b3b-9001-docker:/var/lib/docker"));
    assert!(!w.contains("--hostname"));
    assert!(w.ends_with(" work:dev"));
    assert!(!w.contains("MOOR_KVM_GID"));
}

#[test]
fn the_work_place_joins_the_group_of_a_hosts_kvm_device() {
    let n = Names::of(&slug());
    let w = work_run(&settings(), &slug(), &n, 4096, Some(994), &[]).join(" ");
    assert!(w.contains("--privileged --restart no -e MOOR_KVM_GID=994 --memory"));
}

#[test]
fn a_device_the_host_lacks_has_no_group() {
    let dir = std::env::temp_dir();
    assert_eq!(gid_of(&dir.join("moor-no-such-kvm-device")), None);
    assert!(gid_of(&dir).is_some());
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
    use super::cli::parse_ps;
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

#[test]
fn both_places_mount_the_shared_skills_volumes_and_nothing_else_of_the_home() {
    let n = Names::of(&slug());
    let a = agent_run(&settings(), &slug(), &n, &[]).join(" ");
    let w = work_run(&settings(), &slug(), &n, 4096, None, &[]).join(" ");
    for argv in [&a, &w] {
        assert!(argv.contains("-v moor-3b3b3b3b-9001-agents:/home/agent/.agents "));
        assert!(argv.contains("-v moor-3b3b3b3b-9001-skills:/home/agent/.claude/skills "));
        assert!(argv.contains("-v moor-3b3b3b3b-9001-tmp:/tmp "));
    }
    assert!(a.contains("-v moor-3b3b3b3b-9001-agent-home:/home/agent "));
    assert!(w.contains("-v moor-3b3b3b3b-9001-work-home:/home/agent "));
    assert!(!a.contains("work-home") && !w.contains("agent-home"));
}

#[test]
fn close_removes_the_shared_skills_volumes() {
    let n = Names::of(&slug());
    assert!(n.volumes().contains(&n.agents.as_str()));
    assert!(n.volumes().contains(&n.skills.as_str()));
    assert!(n.volumes().contains(&n.tmp.as_str()));
}

#[test]
fn inspect_lines_say_how_each_container_ended_and_odd_lines_are_left_out() {
    let ended = parse_ended(
        "/moor-3b3b3b3b-9001-agent 143 false\n/moor-3b3b3b3b-9001-work 137 true\nError: No such object\n",
    );
    assert_eq!(ended.len(), 2, "{ended:?}");
    assert_eq!(
        ended.get("moor-3b3b3b3b-9001-agent"),
        Some(&Ended {
            code: 143,
            oom: false
        })
    );
    assert_eq!(
        ended.get("moor-3b3b3b3b-9001-work"),
        Some(&Ended {
            code: 137,
            oom: true
        })
    );
}

#[test]
fn the_docker_provider_keeps_cache_volumes_and_the_work_place_mounts_them() {
    use crate::boxes::provider::BoxProvider;
    assert!(super::Docker::new(Images::default(), None).keeps_caches());
    let cache = vec![
        "-v".to_owned(),
        "moor-cache-0123456789abcdef-go-0:/home/agent/go/pkg/mod".to_owned(),
    ];
    let w = work_run(
        &settings(),
        &slug(),
        &Names::of(&slug()),
        4096,
        None,
        &cache,
    )
    .join(" ");
    assert!(w.contains("-v moor-cache-0123456789abcdef-go-0:/home/agent/go/pkg/mod work:dev"));
}
