//! A runner and a controller of different releases keep reporting links to each other.

use serde::Deserialize;
use serde_json::json;

use crate::request::{MachineLink, MachineLinksReq, RunnerAddr};
use crate::seam::clock::Ts;

/// What a controller one release older read of a link, which ignored keys it did not know.
#[derive(Debug, Deserialize)]
struct OlderLink {
    machine_id: i64,
    online: bool,
    max_boxes: Option<u32>,
}

fn link(provides: Option<&str>) -> MachineLink {
    MachineLink {
        machine_id: 3,
        online: true,
        since: Ts::from_millis(5),
        version: None,
        protocol: None,
        max_boxes: Some(2),
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        memory: None,
        provides: provides.map(str::to_owned),
    }
}

fn report(links: Vec<MachineLink>) -> MachineLinksReq {
    MachineLinksReq {
        runner: RunnerAddr::new("/run/runner.sock"),
        draining: false,
        links,
        box_memory: Vec::new(),
    }
}

#[test]
fn a_link_from_a_runner_that_predates_provides_reads_as_providing_none() {
    let old = json!({"machine_id": 3, "online": true, "since": 5, "max_boxes": 2});
    let read: MachineLink = serde_json::from_value(old).unwrap();
    assert_eq!(read, link(None));
}

#[test]
fn a_link_that_provides_nothing_is_written_as_an_older_runner_wrote_it() {
    let wire = serde_json::to_value(link(None)).unwrap();
    assert!(wire.get("provides").is_none(), "{wire}");
}

#[test]
fn a_link_that_provides_boxes_is_still_read_by_a_controller_that_ignores_unknown_keys() {
    let wire = serde_json::to_value(link(Some("docker"))).unwrap();
    assert_eq!(wire["provides"], "docker");
    let older: OlderLink = serde_json::from_value(wire).unwrap();
    assert_eq!(
        (older.machine_id, older.online, older.max_boxes),
        (3, true, Some(2))
    );
}

#[test]
fn a_report_of_a_link_that_provides_boxes_adds_no_key_a_strict_controller_would_refuse() {
    let wire = serde_json::to_value(report(vec![link(Some("docker"))])).unwrap();
    let keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["runner", "links"]);
    let back: MachineLinksReq = serde_json::from_value(wire).unwrap();
    assert_eq!(back.links[0].provides.as_deref(), Some("docker"));
}
