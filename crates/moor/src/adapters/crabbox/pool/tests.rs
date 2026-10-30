#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use moor_client::link::lease::{Answer, Lease, Verb};
use moor_core::name::{BoxHandle, BoxId, ProviderId};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, Released, SleepOutcome};

use super::PoolHosts;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::ScriptedProc;

const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFFFDh4nOGugXrBRSnPSMPjAYOcCtjIpXx11b21eJfMv";

/// One ask: verb, slug and key.
type Asked = (Verb, Option<String>, Option<String>);

/// A host whose answers are scripted per verb; it records each ask.
#[derive(Default)]
struct FakeHost {
    answers: Mutex<BTreeMap<String, Result<Answer, String>>>,
    asked: Mutex<Vec<Asked>>,
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
    fn machine(&self, host: &str) -> Result<i64, String> {
        if host == "hv" {
            Ok(42)
        } else {
            Err(format!("{host} is not enrolled with moor-machine-agent"))
        }
    }

    fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
    ) -> Result<Answer, String> {
        assert_eq!(machine_id, 42);
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
    let host = Arc::new(FakeHost::default());
    host.answer(Verb::Info, Ok(info("crabbox", HOST_KEY)));
    let cb = Crabbox::fake(Arc::new(ScriptedProc::new()))
        .with_pool_hosts(Arc::clone(&host) as Arc<dyn PoolHosts>);
    (host, cb)
}

fn req() -> AcquireReq {
    let mut req = crate::adapters::crabbox::tests::req("external@hv", None);
    req.box_id = BoxId::new(9101);
    req
}

fn handle() -> BoxHandle {
    BoxHandle::new("external@hv/moor-9101")
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
    assert_eq!(asked[1].1.as_deref(), Some("moor-9101"));

    let words = cb.connect_argv(&handle()).unwrap().join(" ");
    for want in [
        "IdentitiesOnly=yes",
        "StrictHostKeyChecking=yes",
        "HostKeyAlias=moor-9101",
        "ProxyCommand=moor link-dial 42 moor-9101",
        "crabbox@moor-9101",
    ] {
        assert!(words.contains(want), "{want} in {words}");
    }
    let known = words.split("UserKnownHostsFile=").nth(1).unwrap();
    let known = std::fs::read_to_string(known.split(' ').next().unwrap()).unwrap();
    assert_eq!(known, format!("moor-9101 {HOST_KEY}\n"));
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
fn list_keeps_only_moor_leases() {
    let (host, cb) = rig();
    let lease = |slug: &str, state: &str| Lease {
        slug: slug.to_owned(),
        state: state.to_owned(),
    };
    host.answer(
        Verb::List,
        Ok(Answer::Leases {
            leases: vec![
                lease("moor-7", "running"),
                lease("other", "running"),
                lease("moor-8", "paused"),
            ],
        }),
    );
    let got = cb.list(&ProviderId::new("external@hv")).unwrap();
    let names: Vec<(&str, bool)> = got.iter().map(|b| (b.handle.as_str(), b.asleep)).collect();
    assert_eq!(
        names,
        [("external@hv/moor-7", false), ("external@hv/moor-8", true)]
    );
}

#[test]
fn an_unenrolled_host_is_refused_and_caps_need_no_crabbox() {
    let (_, cb) = rig();
    let mut other = req();
    other.provider = ProviderId::new("external@laptop");
    let err = cb.acquire(&other).unwrap_err().to_string();
    assert!(err.contains("not enrolled"), "{err}");
    let caps = cb.caps(&ProviderId::new("external@hv")).unwrap();
    assert!(!caps.reverse_tunnel && caps.wake);
}
