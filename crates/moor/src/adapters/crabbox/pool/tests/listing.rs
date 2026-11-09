//! Each pool host answers only for its own leases, and a slow host costs a listing at most its
//! deadline.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_core::name::BoxHandle;

use super::PoolHosts;
use crate::adapters::crabbox::pool::list_handles;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::ScriptedProc;

/// How a host answers a list: after a delay, with its leases or a failure.
type Reply = (Duration, Result<Vec<&'static str>, &'static str>);

/// Hosts by machine id, each answering a list as scripted.
struct Hosts(BTreeMap<i64, Reply>);

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
        let (delay, reply) = self.0.get(&machine_id).cloned().unwrap();
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

fn pool(hosts: impl IntoIterator<Item = (i64, Reply)>) -> Crabbox {
    let hosts = Arc::new(Hosts(hosts.into_iter().collect()));
    Crabbox::fake(Arc::new(ScriptedProc::new())).with_pool_hosts(Some(hosts as Arc<dyn PoolHosts>))
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
    );
    assert_eq!(asked.answered, [on(1, 11), on(1, 12)]);
    let listed: Vec<_> = asked.listed.iter().map(|l| l.handle.clone()).collect();
    assert_eq!(listed, [on(1, 11)]);
    assert_eq!(
        asked.unanswered,
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
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert_eq!(asked.answered, [on(1, 11), on(3, 31)]);
    assert_eq!(
        asked.unanswered,
        ["pool host 2 did not list its leases within 1s"]
    );
}
