#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use super::args::{agent_run, work_memory_mib, work_run};
use super::cli::{listing_failed, parse_ended, read_state, State};
use super::names::Names;
use super::observe::held_by_box;
use super::{net, Images, Settings};
use crate::boxes::provider::{Ended, ProvideError};
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
            ..Images::default()
        },
        "net",
    )
}

const SOCKET_GONE: &str = "failed to connect to the docker API at unix:///run/user/1003/docker.sock; check if the path is correct and if the daemon is running: dial unix /run/user/1003/docker.sock: connect: no such file or directory";
const NOT_RUNNING: &str = "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?";

#[test]
fn a_container_that_runs_or_is_stopped_is_read_from_its_state() {
    assert_eq!(
        read_state(true, "true\n", "", None).unwrap(),
        Some(State::Running)
    );
    assert_eq!(
        read_state(true, "false\n", "", None).unwrap(),
        Some(State::Stopped)
    );
}

#[test]
fn a_container_docker_does_not_know_is_missing() {
    for said in [
        "Error response from daemon: No such container: moor-3b3b3b3b-9001-agent",
        "Error: No such object: moor-3b3b3b3b-9001-agent",
    ] {
        assert_eq!(read_state(false, "", said, None).unwrap(), None, "{said}");
    }
}

#[test]
fn a_daemon_that_does_not_answer_is_not_a_missing_box_even_when_the_socket_is_gone() {
    for said in [SOCKET_GONE, NOT_RUNNING] {
        let err = read_state(false, "", said, None).unwrap_err().to_string();
        assert!(err.contains("is not answering"), "{err}");
        assert!(!err.contains("does not exist"), "{err}");
    }
}

#[test]
fn the_unanswering_daemons_address_comes_from_docker_then_docker_host() {
    let err = |said, host| read_state(false, "", said, host).unwrap_err().to_string();
    assert!(err(SOCKET_GONE, Some("unix:///elsewhere.sock"))
        .starts_with("the Docker daemon at unix:///run/user/1003/docker.sock is not answering"),);
    assert!(err(NOT_RUNNING, None)
        .starts_with("the Docker daemon at unix:///var/run/docker.sock is not answering"));
    let odd = "error during connect: something broke. Is the docker daemon running?";
    assert!(err(odd, Some("tcp://10.0.0.5:2375"))
        .starts_with("the Docker daemon at tcp://10.0.0.5:2375 is not answering"));
    assert!(err(odd, None).starts_with("the Docker daemon at the default address is not answering"));
}

#[test]
fn a_daemon_that_does_not_answer_loses_every_box_and_any_other_failure_does_not() {
    let ProvideError::Lost(why) = listing_failed(SOCKET_GONE, None) else {
        panic!("a daemon that does not answer loses its Boxes");
    };
    assert!(
        why.starts_with("the Docker daemon at unix:///run/user/1003/docker.sock is not answering"),
        "{why}"
    );
    let e = listing_failed("permission denied", None);
    assert!(matches!(e, ProvideError::Failed(_)), "{e:?}");
}

#[test]
fn any_other_docker_failure_is_said_as_docker_said_it() {
    let err = read_state(false, "", "permission denied while trying to connect", None)
        .unwrap_err()
        .to_string();
    assert_eq!(err, "docker: permission denied while trying to connect");
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
    assert!(!a.contains("--add-host"));
    assert!(a.contains("-v moor-3b3b3b3b-9001-tree:/home/agent/moor"));
    assert!(!a.contains("--privileged"));
    assert!(a.ends_with(" agent:dev"));
}

#[test]
fn the_work_place_joins_the_agent_places_network_and_is_privileged() {
    let n = Names::of(&slug());
    let w = work_run(&settings(), &slug(), &n, 4096, &[]).join(" ");
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
    let w = work_run(&settings(), &slug(), &n, 4096, &[]).join(" ");
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
    let w = work_run(&settings(), &slug(), &Names::of(&slug()), 4096, &cache).join(" ");
    assert!(w.contains("-v moor-cache-0123456789abcdef-go-0:/home/agent/go/pkg/mod work:dev"));
}

#[test]
fn docker_stats_count_both_places_of_a_box_and_skip_other_containers() {
    let stats = [
        r#"{"Name":"moor-1a2b3c4d-417-agent","MemUsage":"512MiB / 30.47GiB"}"#,
        r#"{"Name":"moor-1a2b3c4d-417-work","MemUsage":"2.5GiB / 16GiB"}"#,
        r#"{"Name":"swpa508-volume-mysql","MemUsage":"4.106GiB / 30.47GiB"}"#,
        "not json",
    ]
    .join("\n");
    let held = held_by_box(&stats);
    assert_eq!(
        held.into_iter().collect::<Vec<_>>(),
        [(Slug::parse("moor-1a2b3c4d-417").unwrap(), 3 * 1024)]
    );
}
