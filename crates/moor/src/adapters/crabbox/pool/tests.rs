#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use moor_client::link::lease::{Answer, Verb};
use moor_core::box_key::{ControllerId, PoolSeat};
use moor_core::name::{BoxHandle, BoxId, ProviderId};
use moor_core::seam::driver::{
    AcquireReq, Driver, DriverError, Released, SleepOutcome, TunnelFailure,
};
use moor_core::seam::proc::Tool;

use super::PoolHosts;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{ChildReply, ScriptedProc};

const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFFFDh4nOGugXrBRSnPSMPjAYOcCtjIpXx11b21eJfMv";

/// One ask: verb, slug and key.
type Asked = (Verb, Option<String>, Option<String>);

/// A host whose answers are scripted per verb; it records each ask.
#[derive(Default)]
struct FakeHost {
    answers: Mutex<BTreeMap<String, Result<Answer, String>>>,
    asked: Mutex<Vec<Asked>>,
    machines: Mutex<Vec<i64>>,
}

impl FakeHost {
    fn answer(&self, verb: Verb, answer: Result<Answer, String>) {
        self.answers
            .lock()
            .unwrap()
            .insert(format!("{verb:?}"), answer);
    }

    fn verbs(&self) -> Vec<Verb> {
        self.asked.lock().unwrap().iter().map(|a| a.0).collect()
    }
}

impl PoolHosts for FakeHost {
    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
    ) -> Result<Answer, String> {
        self.machines.lock().unwrap().push(machine_id);
        self.asked
            .lock()
            .unwrap()
            .push((verb, slug.map(str::to_owned), key.map(str::to_owned)));
        self.answers
            .lock()
            .unwrap()
            .get(&format!("{verb:?}"))
            .cloned()
            .unwrap_or(Ok(Answer::Done))
    }

    fn dial_command(&self, machine_id: i64, slug: &str) -> String {
        format!("moor link-dial {machine_id} {slug}")
    }
}

fn info(user: &str, key: &str) -> Answer {
    Answer::Info {
        user: user.to_owned(),
        host_keys: vec![key.to_owned()],
    }
}

fn rig() -> (Arc<FakeHost>, Crabbox) {
    rig_on(Arc::new(ScriptedProc::new()))
}

fn rig_on(proc: Arc<ScriptedProc>) -> (Arc<FakeHost>, Crabbox) {
    let host = Arc::new(FakeHost::default());
    host.answer(Verb::Info, Ok(info("crabbox", HOST_KEY)));
    let cb = Crabbox::fake(proc).with_pool_hosts(Some(Arc::clone(&host) as Arc<dyn PoolHosts>));
    (host, cb)
}

fn req() -> AcquireReq {
    let mut req = crate::adapters::crabbox::tests::req("external@hv", None);
    req.box_id = BoxId::new(9101);
    req.pool_machine = Some(PoolSeat {
        machine: 42,
        controller: ControllerId::from_bytes([0x1a, 0x2b, 0x3c, 0x4d]),
    });
    req
}

fn handle() -> BoxHandle {
    BoxHandle::new("external@hv/42/moor-1a2b3c4d-9101")
}

#[test]
fn acquire_warms_up_lets_the_key_in_and_builds_its_own_words() {
    let (host, cb) = rig();
    let got = cb.acquire(&req()).unwrap();
    assert_eq!(got.handle, handle());
    assert_eq!(got.workdir.as_str(), "/home/crabbox/moor");
    assert_eq!(host.verbs(), [Verb::Warmup, Verb::Authorize, Verb::Info]);
    let asked = host.asked.lock().unwrap().clone();
    let key = asked[1].2.clone().unwrap();
    assert!(key.starts_with("ssh-ed25519 "), "{key}");
    assert!(!key.contains("PRIVATE"), "only the public key leaves");
    assert_eq!(asked[1].1.as_deref(), Some("moor-1a2b3c4d-9101"));

    let words = cb.connect_argv(&handle()).unwrap().join(" ");
    for want in [
        "IdentitiesOnly=yes",
        "StrictHostKeyChecking=yes",
        "HostKeyAlias=moor-1a2b3c4d-9101",
        "ProxyCommand=moor link-dial 42 moor-1a2b3c4d-9101",
        "crabbox@moor-1a2b3c4d-9101",
    ] {
        assert!(words.contains(want), "{want} in {words}");
    }
    let known = words.split("UserKnownHostsFile=").nth(1).unwrap();
    let known = std::fs::read_to_string(known.split(' ').next().unwrap()).unwrap();
    assert_eq!(known, format!("moor-1a2b3c4d-9101 {HOST_KEY}\n"));
}

#[test]
fn a_bad_user_or_host_key_is_refused_and_the_lease_stopped() {
    for answer in [
        Ok(info("Root; rm -rf /", HOST_KEY)),
        Ok(info("crabbox", "ssh-ed25519 not-base64\nevil")),
    ] {
        let (host, cb) = rig();
        host.answer(Verb::Info, answer);
        let err = cb.acquire(&req()).unwrap_err();
        assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
        assert_eq!(host.verbs().last(), Some(&Verb::Stop));
    }
}

#[test]
fn release_stops_and_a_gone_lease_counts_as_deleted() {
    let (host, cb) = rig();
    assert_eq!(cb.release(&handle()).unwrap(), Released::Deleted);
    host.answer(Verb::Stop, Err("machine 42 refused: boom".to_owned()));
    assert!(cb.release(&handle()).is_err());
}

#[test]
fn sleep_and_wake_pause_and_resume_or_say_they_cannot() {
    let (host, cb) = rig();
    assert_eq!(cb.sleep(&handle()).unwrap(), SleepOutcome::Slept);
    cb.wake(&handle()).unwrap();
    assert_eq!(host.verbs(), [Verb::Pause, Verb::Resume]);
    host.answer(Verb::Pause, Ok(Answer::Unsupported));
    host.answer(Verb::Resume, Ok(Answer::Unsupported));
    assert!(matches!(
        cb.sleep(&handle()).unwrap(),
        SleepOutcome::CannotSleep(_)
    ));
    assert!(matches!(
        cb.wake(&handle()),
        Err(DriverError::Unsupported(_))
    ));
}

#[test]
fn the_planned_handle_names_the_host_machine() {
    let (_, cb) = rig();
    assert_eq!(cb.planned_handle(&req()), handle());
}

#[test]
fn every_later_op_reaches_the_machine_its_handle_names() {
    let (host, cb) = rig();
    let on_seven = BoxHandle::new("external@hv/7/moor-1a2b3c4d-9101");
    cb.sleep(&on_seven).unwrap();
    cb.release(&on_seven).unwrap();
    cb.connect_argv(&on_seven).unwrap();
    assert_eq!(*host.machines.lock().unwrap(), [7, 7, 7]);
}

#[test]
fn a_handle_without_a_well_formed_machine_and_slug_is_refused_unasked() {
    let (host, cb) = rig();
    for bad in [
        "external@hv/moor-1a2b3c4d-9101",
        "external@hv/+42/moor-1a2b3c4d-9101",
        "external@hv//moor-1a2b3c4d-9101",
        "external@hv/4x/moor-1a2b3c4d-9101",
        "external@hv/42/other",
        "external@hv/42/moor-9101",
        "external@hv/42/moor-1A2B3C4D-9101",
        "external@hv/42/moor-1a2b3c4d-9101/x",
        "external/42/moor-1a2b3c4d-9101",
    ] {
        assert!(cb.release(&BoxHandle::new(bad)).is_err(), "{bad}");
    }
    assert!(host.verbs().is_empty());
}

#[test]
fn a_pool_provider_is_not_listable() {
    let (host, cb) = rig();
    let got = cb.list(&ProviderId::new("external@hv"));
    assert!(matches!(got, Err(DriverError::Unsupported(_))), "{got:?}");
    assert!(host.verbs().is_empty());
}

#[test]
fn an_acquire_with_no_pool_machine_fails_and_caps_need_no_crabbox() {
    let (host, cb) = rig();
    let mut other = req();
    other.pool_machine = None;
    let err = cb.acquire(&other).unwrap_err().to_string();
    assert!(err.contains("no machine there"), "{err}");
    assert!(host.verbs().is_empty());
    let caps = cb.caps(&ProviderId::new("external@hv")).unwrap();
    assert!(caps.reverse_tunnel && caps.wake);
}

/// A pool Box whose ssh child answers `replies` in order, the last one repeating.
fn tunnel_rig(replies: &[ChildReply]) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let (last, first) = replies.split_last().unwrap();
    for r in first {
        p.once_child(Tool::Ssh, &["-T"], r.clone());
    }
    p.on_child(Tool::Ssh, &["-T"], last.clone());
    let (_, cb) = rig_on(Arc::clone(&p));
    (p, cb)
}

fn refused() -> ChildReply {
    ChildReply::Exit {
        code: 255,
        stderr: "bind [127.0.0.1]:41000: Address already in use\n\
                 Could not request local forwarding."
            .to_owned(),
    }
}

fn ssh_spawns(p: &ScriptedProc) -> usize {
    p.calls().len()
}

#[test]
fn a_pool_local_tunnel_forwards_a_host_port_over_the_pool_words() {
    let (p, cb) = tunnel_rig(&[ChildReply::Ready("ready".to_owned())]);
    let t = cb.local_tunnel(&handle(), 8080).unwrap();
    let args = p.calls()[0].args.clone();
    let words = args.join(" ");
    assert_ne!(t.host_port, 0);
    let forward = format!("-L 127.0.0.1:{}:127.0.0.1:8080 ", t.host_port);
    for want in [
        "-T",
        "ExitOnForwardFailure=yes",
        forward.as_str(),
        "IdentitiesOnly=yes",
        "ProxyCommand=moor link-dial 42 moor-1a2b3c4d-9101",
        "crabbox@moor-1a2b3c4d-9101",
    ] {
        assert!(words.contains(want), "{want} in {words}");
    }
    assert!(t.tunnel.alive());
    let pid = t.tunnel.pid();
    drop(t);
    assert_eq!(p.dropped(), [pid]);
}

#[test]
fn a_pool_local_tunnel_that_loses_its_port_tries_a_second_one() {
    let (p, cb) = tunnel_rig(&[refused(), ChildReply::Ready("ready".to_owned())]);
    let t = cb.local_tunnel(&handle(), 8080).unwrap();
    assert!(t.tunnel.alive());
    assert_eq!(ssh_spawns(&p), 2);
}

#[test]
fn a_pool_local_tunnel_that_cannot_forward_is_an_error_after_one_retry() {
    let (p, cb) = tunnel_rig(&[refused()]);
    let err = cb.local_tunnel(&handle(), 8080).unwrap_err();
    assert!(
        matches!(
            err,
            DriverError::Tunnel {
                failure: TunnelFailure::Exited {
                    code: Some(255),
                    ..
                },
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(ssh_spawns(&p), 2);
}

#[test]
fn a_pool_local_tunnel_failing_for_another_reason_is_not_retried() {
    for reply in [
        ChildReply::Exit {
            code: 255,
            stderr: "Permission denied (publickey).".to_owned(),
        },
        ChildReply::Hang,
        ChildReply::Ready("nothing like it".to_owned()),
    ] {
        let (p, cb) = tunnel_rig(&[reply]);
        assert!(cb.local_tunnel(&handle(), 8080).is_err());
        assert_eq!(ssh_spawns(&p), 1);
        assert_eq!(p.dropped().len(), 1);
    }
}
