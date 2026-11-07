#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::sync::{Arc, Mutex};

use moor_client::link::lease::{Answer, Boot, CheckpointAct, Verb};
use moor_core::box_key::{ControllerId, PoolSeat};
use moor_core::domain::event::MAX_STEPS;
use moor_core::name::{BoxHandle, BoxId, ProviderId};
use moor_core::seam::driver::{Driver, DriverCaps};

use crate::adapters::crabbox::pool::PoolHosts;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::ScriptedProc;

const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFFFDh4nOGugXrBRSnPSMPjAYOcCtjIpXx11b21eJfMv";
const SLUG: &str = "moor-1a2b3c4d-9101";

/// A host that records what it is asked.
#[derive(Default)]
struct Host {
    acts: Mutex<Vec<(i64, CheckpointAct)>>,
    verbs: Mutex<Vec<Verb>>,
    refuse_acts: bool,
    refuse_authorize: bool,
    wrong_answer: bool,
    fork_timing: Option<Vec<String>>,
}

impl PoolHosts for Host {
    fn lease(
        &self,
        _: i64,
        verb: Verb,
        _: Option<&str>,
        _: Option<&str>,
        _: &Boot,
    ) -> Result<Answer, String> {
        self.verbs.lock().unwrap().push(verb);
        match verb {
            Verb::Authorize if self.refuse_authorize => Err("no key for you".to_owned()),
            Verb::Info => Ok(Answer::Info {
                user: "crabbox".to_owned(),
                host_keys: vec![HOST_KEY.to_owned()],
            }),
            _ => Ok(Answer::Done),
        }
    }

    fn checkpoint(&self, machine: i64, act: CheckpointAct) -> Result<Answer, String> {
        self.acts.lock().unwrap().push((machine, act.clone()));
        if self.refuse_acts {
            return Err("the host refused".to_owned());
        }
        if self.wrong_answer {
            return Ok(Answer::Unsupported);
        }
        Ok(match act {
            CheckpointAct::CheckpointCreate { .. } => Answer::Checkpoint {
                id: "chk_1".to_owned(),
                bytes: Some(5),
                parent: Some("chk_0".to_owned()),
            },
            CheckpointAct::Disk {} => Answer::Disk {
                free_mib: 7,
                total_mib: 70,
            },
            CheckpointAct::CheckpointFork { .. } => self
                .fork_timing
                .clone()
                .map_or(Answer::Done, |timing| Answer::Forked { timing }),
            CheckpointAct::CheckpointDelete { .. } => Answer::Done,
        })
    }

    fn dial_command(&self, machine: i64, slug: &str) -> Result<String, String> {
        Ok(format!("moor link-dial {machine} {slug}"))
    }
}

fn rig(host: Host) -> (Arc<Host>, Crabbox, tempfile::TempDir) {
    let host = Arc::new(host);
    let dir = tempfile::tempdir().unwrap();
    let cb = Crabbox::fake(Arc::new(ScriptedProc::new()))
        .with_pool_hosts(Some(Arc::clone(&host) as Arc<dyn PoolHosts>));
    (host, cb, dir)
}

fn req() -> moor_core::seam::driver::AcquireReq {
    let mut req = crate::adapters::crabbox::tests::req("external@hv", Some("medium"));
    req.box_id = BoxId::new(9101);
    req.pool_machine = Some(PoolSeat {
        machine: 42,
        controller: ControllerId::from_bytes([0x1a, 0x2b, 0x3c, 0x4d]),
    });
    req
}

fn handle() -> BoxHandle {
    BoxHandle::new(format!("external@hv/42/{SLUG}"))
}

fn home() -> ProviderId {
    ProviderId::new("external@hv/42")
}

#[test]
fn a_pool_provider_offers_checkpoints_for_its_agent_to_confirm() {
    let (_, cb, _dir) = rig(Host::default());
    let caps: DriverCaps = cb.caps(&ProviderId::new("external@hv")).unwrap();
    assert!(caps.checkpoints);
}

#[test]
fn a_save_goes_to_the_machine_of_the_box_and_comes_back_as_data() {
    let (host, cb, _dir) = rig(Host::default());
    let made = cb.checkpoint(&handle(), "layer").unwrap();
    assert_eq!(made.id, "chk_1");
    assert_eq!(made.bytes, Some(5));
    assert_eq!(made.parent.as_deref(), Some("chk_0"));
    let acts = host.acts.lock().unwrap();
    assert_eq!(
        acts.as_slice(),
        [(
            42,
            CheckpointAct::CheckpointCreate {
                slug: SLUG.to_owned(),
                name: "layer".to_owned(),
            }
        )]
    );
}

#[test]
fn a_fork_boots_on_the_seats_machine_then_authorizes_and_reads_the_vm() {
    let (host, cb, _dir) = rig(Host::default());
    let got = cb.fork("chk_1", &req()).unwrap();
    assert_eq!(got.handle.as_str(), handle().as_str());
    let acts = host.acts.lock().unwrap();
    assert_eq!(
        acts.as_slice(),
        [(
            42,
            CheckpointAct::CheckpointFork {
                id: "chk_1".to_owned(),
                slug: SLUG.to_owned(),
                cache: None,
                class: Some("fast".to_owned()),
            }
        )]
    );
    let verbs = host.verbs.lock().unwrap();
    assert_eq!(verbs.as_slice(), [Verb::Authorize, Verb::Info]);
}

#[test]
fn a_fork_reads_the_steps_the_host_reports_and_drops_what_it_cannot_read() {
    let mut lines = vec![
        "timing agent-room ms=3".to_owned(),
        "not a step".to_owned(),
        "timing root-flatten ms=7641 bytes=5".to_owned(),
    ];
    lines.extend(vec!["timing n ms=1".to_owned(); MAX_STEPS * 2]);
    let (_, cb, _dir) = rig(Host {
        fork_timing: Some(lines),
        ..Host::default()
    });
    let steps = cb.fork("chk_1", &req()).unwrap().steps;
    assert_eq!(steps.len(), MAX_STEPS);
    let got: Vec<_> = steps
        .iter()
        .take(2)
        .map(|s| (s.name.as_str(), s.ms, s.detail.as_str()))
        .collect();
    assert_eq!(
        got,
        [("agent-room", 3, ""), ("root-flatten", 7641, "bytes=5")]
    );
}

#[test]
fn a_fork_from_an_older_agent_has_no_steps() {
    let (_, cb, _dir) = rig(Host::default());
    assert_eq!(cb.fork("chk_1", &req()).unwrap().steps, []);
}

#[test]
fn a_fork_refused_by_the_host_is_a_failure_and_stops_nothing() {
    let (host, cb, _dir) = rig(Host {
        refuse_acts: true,
        ..Host::default()
    });
    let e = cb.fork("chk_1", &req()).unwrap_err();
    assert!(e.to_string().contains("the host refused"), "{e}");
    assert!(host.verbs.lock().unwrap().is_empty());
}

#[test]
fn a_fork_the_controller_cannot_reach_is_stopped() {
    let (host, cb, _dir) = rig(Host {
        refuse_authorize: true,
        ..Host::default()
    });
    assert!(cb.fork("chk_1", &req()).is_err());
    let verbs = host.verbs.lock().unwrap();
    assert_eq!(verbs.as_slice(), [Verb::Authorize, Verb::Stop]);
}

#[test]
fn a_fork_with_no_seat_asks_no_host() {
    let (host, cb, _dir) = rig(Host::default());
    let mut asked = req();
    asked.pool_machine = None;
    assert!(cb.fork("chk_1", &asked).is_err());
    assert!(host.acts.lock().unwrap().is_empty());
}

#[test]
fn a_delete_and_a_disk_read_go_to_the_machine_the_home_names() {
    let (host, cb, _dir) = rig(Host::default());
    cb.delete_checkpoint(&home(), "chk_1").unwrap();
    let disk = cb.checkpoint_disk(&home()).unwrap().unwrap();
    assert_eq!((disk.free_mib, disk.total_mib), (7, 70));
    let acts = host.acts.lock().unwrap();
    assert_eq!(
        acts.as_slice(),
        [
            (
                42,
                CheckpointAct::CheckpointDelete {
                    id: "chk_1".to_owned()
                }
            ),
            (42, CheckpointAct::Disk {})
        ]
    );
}

#[test]
fn a_provider_with_no_machine_never_asks_a_host_about_its_disk() {
    let (host, cb, _dir) = rig(Host::default());
    assert_eq!(
        cb.checkpoint_disk(&ProviderId::new("external@hv")).unwrap(),
        None
    );
    assert!(host.acts.lock().unwrap().is_empty());
}

#[test]
fn an_answer_that_does_not_fit_the_verb_is_a_failure() {
    let (_, cb, _dir) = rig(Host {
        wrong_answer: true,
        ..Host::default()
    });
    assert!(cb.checkpoint(&handle(), "layer").is_err());
    assert!(cb.delete_checkpoint(&home(), "chk_1").is_err());
    assert!(cb.checkpoint_disk(&home()).is_err());
    assert!(cb.fork("chk_1", &req()).is_err());
}
