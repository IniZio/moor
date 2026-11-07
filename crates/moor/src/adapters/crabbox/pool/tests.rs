#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use moor_client::link::lease::{Answer, Boot, Cache, Lease, Verb};
use moor_core::box_key::{ControllerId, PoolSeat};
use moor_core::cache::{CacheLayers, CachePlan, CacheSalt};
use moor_core::config::parse_repo;
use moor_core::name::{BoxHandle, BoxId, ProviderId};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, Released, SleepOutcome};
use moor_core::user::UserId;

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
    machines: Mutex<Vec<i64>>,
    caches: Mutex<Vec<Option<Cache>>>,
    classes: Mutex<Vec<Option<String>>>,
    /// The runner its link row names; none leaves `--runner` out of the dial.
    runner: Mutex<Option<String>>,
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
        boot: &Boot,
    ) -> Result<Answer, String> {
        self.machines.lock().unwrap().push(machine_id);
        self.caches.lock().unwrap().push(boot.cache.clone());
        self.classes.lock().unwrap().push(boot.class.clone());
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

    fn dial_command(&self, machine_id: i64, slug: &str) -> Result<String, String> {
        let runner = self.runner.lock().unwrap().clone();
        let runner = runner.map(|r| format!("--runner {r} ")).unwrap_or_default();
        Ok(format!("moor link-dial {runner}{machine_id} {slug}"))
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
fn a_pool_lease_is_taken_over_by_an_adopt_not_another_warmup() {
    let (host, cb) = rig();
    assert!(cb.acquire_outlives_caller(&handle()));
    assert!(!cb.acquire_outlives_caller(&BoxHandle::new("p/moor-9-abc")));
    let got = cb.adopt_noting(&req(), &mut |_| {}).unwrap();
    assert_eq!(got.handle, handle());
    assert_eq!(host.verbs(), [Verb::Adopt, Verb::Authorize, Verb::Info]);
}

#[test]
fn a_host_out_of_reach_is_a_lost_link_and_a_refusal_is_not() {
    for (said, lost) in [
        ("machine 42 is not linked", true),
        (
            "machine 42 is offline: its link closed: the stream ended: eof",
            true,
        ),
        (
            "the machine runner at /s/links.sock is unreachable: refused",
            true,
        ),
        ("the stream ended: unexpected end of file", true),
        (
            "machine 42 refused: crabbox warmup failed: the stream ended",
            false,
        ),
        ("no room", false),
    ] {
        let got = super::lost_or_failed(said.to_owned());
        assert_eq!(matches!(got, DriverError::Transport { .. }), lost, "{said}");
    }
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
    assert_eq!(
        cb.sleep(&handle(), moor_core::seam::driver::PausePublish::Denied)
            .unwrap(),
        SleepOutcome::Slept
    );
    cb.wake(&handle()).unwrap();
    assert_eq!(host.verbs(), [Verb::Pause, Verb::Resume]);
    host.answer(Verb::Pause, Ok(Answer::Unsupported));
    host.answer(Verb::Resume, Ok(Answer::Unsupported));
    assert!(matches!(
        cb.sleep(&handle(), moor_core::seam::driver::PausePublish::Denied)
            .unwrap(),
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
    cb.sleep(&on_seven, moor_core::seam::driver::PausePublish::Denied)
        .unwrap();
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
fn a_host_lists_its_stopped_leases_asleep_and_leaves_unknown_ones_out() {
    let (host, cb) = rig();
    let lease = |slug: &str, state: &str| Lease {
        slug: slug.to_owned(),
        state: state.to_owned(),
    };
    host.answer(
        Verb::List,
        Ok(Answer::Leases {
            leases: vec![
                lease("moor-1a2b3c4d-9101", "stopped"),
                lease("moor-1a2b3c4d-9102", "ready"),
            ],
        }),
    );
    let running = BoxHandle::new("external@hv/42/moor-1a2b3c4d-9102");
    let unknown = BoxHandle::new("external@hv/42/moor-1a2b3c4d-9103");
    let listed = cb
        .list_handles(&[handle(), running.clone(), unknown])
        .unwrap();
    let got: Vec<_> = listed
        .iter()
        .map(|l| (l.handle.clone(), l.asleep))
        .collect();
    assert_eq!(got, [(handle(), true), (running, false)]);
    assert_eq!(host.verbs(), [Verb::List]);
}

#[test]
fn an_unreachable_host_lists_none_of_its_leases() {
    let (host, cb) = rig();
    host.answer(Verb::List, Err("not linked".to_owned()));
    assert!(cb.list_handles(&[handle()]).unwrap().is_empty());
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

#[test]
fn acquire_hands_the_box_cache_to_the_host_warmup_only() {
    let (host, cb) = rig();
    let mut req = req();
    let repo = moor_core::name::ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    let layers = CacheLayers {
        repo: parse_repo(".moor.yaml", "cache: {presets: [rust]}")
            .unwrap()
            .cache,
        ..CacheLayers::default()
    };
    req.cache = CachePlan::build(
        UserId::new(1),
        Some(&repo),
        &layers,
        &CacheSalt::new([3; 64]),
    )
    .unwrap();
    cb.acquire(&req).unwrap();

    let caches = host.caches.lock().unwrap().clone();
    let sent = caches[0].clone().expect("the warmup carries the cache");
    assert_eq!(
        serde_json::to_string(&sent.volumes).ok(),
        super::super::cache::env_value(&req.cache)
    );
    assert_eq!(sent.scope.as_deref(), req.cache.scope());
    assert!(caches[1..].iter().all(Option::is_none), "{caches:?}");
}

#[test]
fn a_box_that_keeps_its_work_tree_works_in_the_cached_tree() {
    let (_host, cb) = rig();
    let mut req = req();
    let repo = moor_core::name::ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    req.cache = CachePlan::build(
        UserId::new(1),
        Some(&repo),
        &CacheLayers::default(),
        &CacheSalt::new([3; 64]),
    )
    .unwrap();
    let tree = req
        .cache
        .volumes()
        .iter()
        .find(|v| v.name.as_str() == "worktree");
    let mounted = tree.expect("the plan keeps the work tree").paths[0].as_str();
    assert_eq!(mounted, "~/moor/repos/r");

    let got = cb.acquire(&req).unwrap();
    assert_eq!(got.workdir.as_str(), "/home/crabbox/moor/repos/r");
}

#[test]
fn acquire_of_a_cold_box_sends_no_cache() {
    let (host, cb) = rig();
    cb.acquire(&req()).unwrap();
    assert!(host.caches.lock().unwrap().iter().all(Option::is_none));
}

#[test]
fn a_sized_box_boots_the_class_its_size_asks_for() {
    let (host, cb) = rig();
    let mut sized = req();
    sized.class = Some("medium".to_owned());
    cb.acquire(&sized).unwrap();
    let classes = host.classes.lock().unwrap().clone();
    assert_eq!(classes[0].as_deref(), Some("fast"));
    assert!(classes[1..].iter().all(Option::is_none), "{classes:?}");
}

#[test]
fn a_box_with_no_size_leaves_the_class_to_the_host() {
    let (host, cb) = rig();
    cb.acquire(&req()).unwrap();
    assert!(host.classes.lock().unwrap().iter().all(Option::is_none));
}

#[test]
fn an_unknown_size_fails_before_any_lease() {
    let (host, cb) = rig();
    let mut sized = req();
    sized.class = Some("huge".to_owned());
    let err = cb.acquire(&sized).unwrap_err();
    assert!(err.to_string().contains("unknown box size"), "{err}");
    assert!(host.verbs().is_empty());
}

#[test]
fn a_class_the_host_refuses_fails_the_acquire_with_its_reason() {
    let (host, cb) = rig();
    host.answer(
        Verb::Warmup,
        Err("crabbox warmup failed (Some(2)): unknown class fast".to_owned()),
    );
    let mut sized = req();
    sized.class = Some("medium".to_owned());
    let err = cb.acquire(&sized).unwrap_err();
    assert!(err.to_string().contains("unknown class fast"), "{err}");
    assert_eq!(
        host.verbs().iter().filter(|v| **v == Verb::Warmup).count(),
        1
    );
}

mod fresh_login;
mod tunnel;

#[test]
fn an_allowed_sleep_asks_the_host_to_pause_and_publish() {
    use moor_core::seam::driver::PausePublish::{Allowed, Denied};
    let (host, cb) = rig();
    assert_eq!(cb.sleep(&handle(), Allowed).unwrap(), SleepOutcome::Slept);
    assert_eq!(cb.sleep(&handle(), Denied).unwrap(), SleepOutcome::Slept);
    assert_eq!(host.verbs(), [Verb::PausePublish, Verb::Pause]);
    host.answer(Verb::PausePublish, Ok(Answer::Unsupported));
    assert!(matches!(
        cb.sleep(&handle(), Allowed).unwrap(),
        SleepOutcome::CannotSleep(_)
    ));
}

#[test]
fn a_box_stays_reachable_with_the_same_key_after_an_empty_state_dir_and_a_restart() {
    let (host, cb) = rig();
    let state = cb.scratch.as_ref().unwrap().path().to_owned();
    let kept = tempfile::tempdir().unwrap();
    let clock = Arc::new(crate::testing::clock::FakeClock::new(
        moor_core::seam::clock::Ts::from_millis(0),
    ));
    let store = crate::store::db::testing::open_store(&kept.path().join("moor.db"), clock);
    let vault = crate::vault::Vault::load_or_create(&kept.path().join("vault.key")).unwrap();

    crate::keys::settle(&store, &vault, &state, false).unwrap();
    cb.acquire(&req()).unwrap();
    let key = std::fs::read_to_string(state.join("pool/id_ed25519")).unwrap();

    std::fs::remove_dir_all(&state).unwrap();
    crate::keys::settle(&store, &vault, &state, false).unwrap();
    cb.acquire(&req()).unwrap();

    let authorized: Vec<_> = host
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|a| a.0 == Verb::Authorize)
        .map(|a| a.2.clone())
        .collect();
    assert_eq!(authorized.len(), 2);
    assert_eq!(authorized[0], authorized[1], "Boxes trust the first key");
    assert_eq!(
        std::fs::read_to_string(state.join("pool/id_ed25519")).unwrap(),
        key
    );
}
