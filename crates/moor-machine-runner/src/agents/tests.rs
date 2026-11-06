#![allow(clippy::unwrap_used, reason = "tests assert on fixed fixtures")]

use std::sync::Mutex;

use moor_client::link::{BoxMemory, Offer, Report};
use moor_core::request::{MachineLinksReq, RunnerAddr};
use tokio::sync::{watch, Notify};

use super::Links;

fn links() -> (Links, watch::Receiver<MachineLinksReq>) {
    let (report, rx) = watch::channel(MachineLinksReq {
        runner: RunnerAddr::new("/run/runner.sock"),
        draining: false,
        links: Vec::new(),
        box_memory: Vec::new(),
    });
    let links = Links {
        held: Mutex::default(),
        report,
        next: Mutex::new(0),
        fresh: Notify::new(),
        stopping: watch::Sender::new(false),
    };
    (links, rx)
}

fn report(boxes: &[(&str, u64)]) -> Report {
    Report {
        mem_available_mib: 1024,
        mem_total_mib: 4096,
        psi_some_avg10: None,
        psi_full_avg10: None,
        disk_free_mib: None,
        disk_total_mib: None,
        boxes: boxes
            .iter()
            .map(|(slug, mib)| BoxMemory {
                slug: (*slug).to_owned(),
                host_memory_mib: *mib,
                resume_memory_mib: None,
            })
            .collect(),
    }
}

fn held(rx: &watch::Receiver<MachineLinksReq>) -> Vec<(i64, i64, u64)> {
    rx.borrow()
        .box_memory
        .iter()
        .map(|b| (b.machine_id, b.box_id.get(), b.host_memory_mib))
        .collect()
}

#[test]
fn a_report_stores_the_memory_of_each_box_and_a_later_one_without_it_clears_it() {
    let (links, rx) = links();
    let (generation, _replaced, _asks) = links.up(
        5,
        "1.0.0".to_owned(),
        Offer::default(),
        tokio::sync::mpsc::channel(1).0,
    );
    links.reported(
        5,
        generation,
        &report(&[
            ("moor-0badcafe-7", 1536),
            ("moor-0badcafe-8", 0),
            ("not-a-lease", 9),
        ]),
    );
    assert_eq!(held(&rx), [(5, 7, 1536), (5, 8, 0)]);
    links.reported(5, generation, &report(&[("moor-0badcafe-8", 0)]));
    assert_eq!(held(&rx), [(5, 8, 0)]);
    links.reported(5, generation, &report(&[]));
    assert_eq!(held(&rx), []);
}

#[test]
fn a_machine_that_goes_down_holds_no_box_memory() {
    let (links, rx) = links();
    let (generation, _replaced, _asks) = links.up(
        5,
        "1.0.0".to_owned(),
        Offer::default(),
        tokio::sync::mpsc::channel(1).0,
    );
    links.reported(5, generation, &report(&[("moor-0badcafe-7", 100)]));
    links.down(5, generation);
    assert_eq!(held(&rx), []);
}

#[tokio::test]
async fn a_draining_runner_says_so_with_every_report_after_and_tells_its_agents_to_relink() {
    let (links, mut rx) = links();
    let agents = super::Agents {
        client: moor_client::Client::new(std::path::Path::new("/nonexistent")),
        links: std::sync::Arc::new(links),
        routes: std::sync::Arc::default(),
    };
    let (tell, mut told) = tokio::sync::mpsc::channel(1);
    let (generation, _replaced, _asks) =
        agents
            .links
            .up(5, "1.0.0".to_owned(), Offer::default(), tell);
    agents.drain(std::time::Duration::ZERO).await;
    assert!(matches!(
        told.try_recv(),
        Ok(moor_client::link::Frame::Relink {})
    ));
    agents.links.reported(5, generation, &report(&[]));
    let sent = rx.borrow_and_update();
    assert!(sent.draining);
    assert_eq!(sent.runner.as_str(), "/run/runner.sock");
    assert_eq!(sent.links.len(), 1);
}
