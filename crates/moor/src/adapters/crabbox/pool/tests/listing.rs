//! Each pool host answers only for its own leases, and a slow host costs a listing at most its
//! deadline.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_core::name::BoxHandle;

use super::PoolHosts;
use crate::adapters::crabbox::pool::list_handles;
use crate::adapters::crabbox::pool::HostLists;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::ScriptedProc;

/// How a host answers a list: after a delay, with its leases or a failure.
type Reply = (Duration, Result<Vec<&'static str>, &'static str>);

/// Hosts by machine id, each answering a list as scripted now; each ask is counted.
#[derive(Default)]
struct Hosts {
    replies: Mutex<BTreeMap<i64, Reply>>,
    asks: Mutex<Vec<i64>>,
}

impl Hosts {
    fn set(&self, machine: i64, reply: Reply) {
        self.replies.lock().unwrap().insert(machine, reply);
    }
}

impl PoolHosts for Hosts {
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        _slug: Option<&str>,
        _key: Option<&str>,
        _boot: &Boot,
    ) -> Result<Answer, String> {
        assert_eq!(verb, Verb::List);
        self.asks.lock().unwrap().push(machine_id);
        let (delay, reply) = self
            .replies
            .lock()
            .unwrap()
            .get(&machine_id)
            .cloned()
            .unwrap();
        thread::sleep(delay);
        let leases = reply.map_err(str::to_owned)?;
        Ok(Answer::Leases {
            leases: leases
                .into_iter()
                .map(|slug| Lease {
                    slug: slug.to_owned(),
                    state: "ready".to_owned(),
                })
                .collect(),
        })
    }

    fn dial_command(&self, _machine_id: i64, _slug: &str) -> Result<String, String> {
        Err("no dials".to_owned())
    }
}

/// A driver per pass, as the controller wires one per tick, over `hosts` and `lists`.
fn driver(hosts: &Arc<Hosts>, lists: &Arc<HostLists>) -> Crabbox {
    Crabbox::fake(Arc::new(ScriptedProc::new()))
        .with_pool_hosts(Some(Arc::clone(hosts) as Arc<dyn PoolHosts>))
        .sharing_host_lists(Arc::clone(lists))
}

fn pool(hosts: impl IntoIterator<Item = (i64, Reply)>) -> Crabbox {
    let fake = Arc::new(Hosts::default());
    for (machine, reply) in hosts {
        fake.set(machine, reply);
    }
    driver(&fake, &Arc::default())
}

fn on(machine: i64, id: u32) -> BoxHandle {
    BoxHandle::new(format!("external@hv/{machine}/moor-1a2b3c4d-{id}"))
}

const NOW: Duration = Duration::ZERO;

#[test]
fn a_host_that_answers_speaks_for_its_own_leases_and_an_away_one_for_none() {
    let cb = pool([
        (1, (NOW, Ok(vec!["moor-1a2b3c4d-11"]))),
        (2, (NOW, Err("machine 2 is offline: its link closed"))),
    ]);
    let asked = list_handles(
        &cb,
        &[on(1, 11), on(1, 12), on(2, 21)],
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(asked.answered, [on(1, 11), on(1, 12)]);
    let listed: Vec<_> = asked.listed.iter().map(|l| l.handle.clone()).collect();
    assert_eq!(listed, [on(1, 11)]);
    assert_eq!(
        asked.said,
        ["pool host 2 did not list its leases: machine 2 is offline: its link closed"]
    );
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "measures the wall time a listing takes against real host delays"
)]
fn hosts_are_asked_at_once_and_a_late_one_is_left_unknown_at_the_deadline() {
    let slow = Duration::from_secs(30);
    let cb = pool([
        (1, (Duration::from_millis(700), Ok(vec![]))),
        (2, (slow, Ok(vec![]))),
        (
            3,
            (Duration::from_millis(700), Ok(vec!["moor-1a2b3c4d-31"])),
        ),
    ]);
    let started = Instant::now();
    let asked = list_handles(
        &cb,
        &[on(1, 11), on(2, 21), on(3, 31)],
        Duration::from_secs(1),
    )
    .unwrap();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert_eq!(asked.answered, [on(1, 11), on(3, 31)]);
    assert_eq!(
        asked.said,
        ["pool host 2 did not list its leases within 1s"]
    );
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "measures the wall time a listing takes against real host delays"
)]
fn the_deadline_counts_from_the_ask_not_from_each_answer() {
    let cb = pool([
        (1, (Duration::from_millis(400), Ok(vec![]))),
        (2, (Duration::from_millis(800), Ok(vec![]))),
        (3, (Duration::from_millis(1400), Ok(vec![]))),
    ]);
    let started = Instant::now();
    let asked = list_handles(
        &cb,
        &[on(1, 11), on(2, 21), on(3, 31)],
        Duration::from_secs(1),
    )
    .unwrap();
    let took = started.elapsed();
    assert_eq!(asked.answered, [on(1, 11), on(2, 21)]);
    assert!(took < Duration::from_millis(1300), "{took:?}");
}

#[test]
fn a_host_still_answering_its_last_list_is_not_asked_again() {
    let hosts = Arc::new(Hosts::default());
    hosts.set(1, (Duration::from_secs(3600), Ok(vec![])));
    hosts.set(2, (NOW, Ok(vec![])));
    let lists = Arc::new(HostLists::default());
    for _ in 0..4 {
        let asked = list_handles(
            &driver(&hosts, &lists),
            &[on(1, 11), on(2, 21)],
            Duration::from_millis(100),
        )
        .unwrap();
        assert_eq!(asked.answered, [on(2, 21)]);
    }
    let asks = hosts.asks.lock().unwrap().clone();
    assert_eq!(asks.iter().filter(|m| **m == 1).count(), 1, "{asks:?}");
    assert_eq!(asks.iter().filter(|m| **m == 2).count(), 4, "{asks:?}");
}

#[test]
fn a_host_that_stays_away_is_reported_once_and_again_when_it_answers() {
    let hosts = Arc::new(Hosts::default());
    hosts.set(1, (NOW, Err("machine 1 is not linked")));
    let lists = Arc::new(HostLists::default());
    let pass = || {
        list_handles(
            &driver(&hosts, &lists),
            &[on(1, 11)],
            Duration::from_secs(5),
        )
        .unwrap()
        .said
    };
    assert_eq!(
        pass(),
        ["pool host 1 did not list its leases: machine 1 is not linked"]
    );
    assert!(pass().is_empty());
    assert!(pass().is_empty());
    hosts.set(1, (NOW, Ok(vec!["moor-1a2b3c4d-11"])));
    assert_eq!(pass(), ["pool host 1 lists its leases again"]);
    assert!(pass().is_empty());
    hosts.set(1, (NOW, Err("machine 1 is not linked")));
    assert_eq!(
        pass(),
        ["pool host 1 did not list its leases: machine 1 is not linked"]
    );
}

#[test]
fn a_lease_the_host_no_longer_lists_is_answered_and_left_out() {
    let cb = pool([(1, (NOW, Ok(vec!["moor-d31547ed-125", "moor-d31547ed-122"])))]);
    let gone = BoxHandle::new("external@engine-03/1/moor-d31547ed-81");
    let kept = BoxHandle::new("external@engine-03/1/moor-d31547ed-122");
    let asked = list_handles(&cb, &[gone.clone(), kept.clone()], Duration::from_secs(5)).unwrap();
    assert_eq!(asked.answered, [gone, kept.clone()]);
    let listed: Vec<_> = asked.listed.iter().map(|l| l.handle.clone()).collect();
    assert_eq!(listed, [kept]);
    assert!(asked.said.is_empty(), "{:?}", asked.said);
}
