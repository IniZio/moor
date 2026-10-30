//! Tests for the crabbox adapter.

use std::sync::Arc;
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, Released};
use moor_core::seam::proc::{Proc, Tool};

use super::parse::{parse_list, parse_warmup};
use super::Crabbox;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::rng::FakeRng;

pub(super) mod fake_ssh;
mod lookup;
mod master;
mod pinned;
mod slug;
mod watch;
mod workdir;

pub(super) const WARMUP_OUT: &str =
    "leased cbx_1 slug=moor-t7 provider=sprites server=1 type=t ip=1.2.3.4\n\
ready ssh=u@h:22 network=public workroot=/root/crabbox\n";

pub(super) fn req(provider: &str, class: Option<&str>) -> AcquireReq {
    AcquireReq {
        pool_machine: None,
        provider: ProviderId::new(provider),
        box_id: BoxId::new(7),
        name: BoxName::parse_allow_bare("t7").unwrap(),
        class: class.map(str::to_owned),
        nonce: "ab12cd".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    }
}

impl Crabbox {
    /// A driver on a fake clock that passed the version check and lacks `ssh --no-probe`.
    pub(crate) fn fake(proc: Arc<dyn Proc>) -> Self {
        let driver = Self::fake_unchecked(proc);
        let _ = driver.version_ok.set(());
        let _ = driver.no_probe.set(false);
        driver
    }

    /// Like [`Self::fake`], but still asks crabbox for its version.
    pub(crate) fn fake_unchecked(proc: Arc<dyn Proc>) -> Self {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
        Self::new(proc, clock, Arc::new(FakeRng::default()))
            .with_scratch_leases()
            .with_pinned_options()
    }

    /// Answers every `ssh -G` read as crabbox's pinned words do, for a driver whose ssh is
    /// scripted.
    pub(crate) fn with_pinned_options(mut self) -> Self {
        let options = ScriptedProc::new();
        options.on(
            Tool::Ssh,
            &["-G"],
            Reply::Ok(
                "stricthostkeychecking true\nuserknownhostsfile /lease/known_hosts\n".to_owned(),
            ),
        );
        self.options = Arc::new(options);
        self
    }

    /// Runs lease calls from dirs under a fresh temporary state dir.
    pub(crate) fn with_scratch_leases(mut self) -> Self {
        let dir = Arc::new(tempfile::tempdir().expect("temp state dir"));
        self.leases = Some(dir.path().join("leases"));
        self.scratch = Some(dir);
        self
    }
}

fn fixture() -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn fail(code: i32, stderr: &str) -> Reply {
    Reply::Fail {
        code,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

#[test]
fn warmup_text_parses_id_and_workroot() {
    let w = parse_warmup(WARMUP_OUT).unwrap();
    assert_eq!(w.id, "cbx_1");
    assert_eq!(w.workroot.as_deref(), Some("/root/crabbox"));
}

#[test]
fn warmup_without_ready_has_no_workroot() {
    let w = parse_warmup("noise\nleased cbx_2 slug=-\n").unwrap();
    assert_eq!(w.id, "cbx_2");
    assert_eq!(w.workroot, None);
}

#[test]
fn warmup_without_leased_is_none() {
    assert!(parse_warmup("ready ssh=u@h:22 workroot=/r\n").is_none());
}

#[test]
fn list_prefers_slug_then_name_then_id() {
    let json = r#"[
        {"id": 1, "name": "n1", "status": "running", "labels": {"slug": "s1"}, "extra": 3},
        {"id": "x2", "name": "n2", "status": "off"},
        {"id": 3, "provider": "ssh"},
        {"status": "running"}
    ]"#;
    let items = parse_list(json).unwrap();
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["s1", "n2", "3"]);
    assert!(!items.first().unwrap().asleep);
    assert!(items.get(1).unwrap().asleep);
    assert_eq!(items.get(2).unwrap().provider.as_deref(), Some("ssh"));
}

#[test]
fn list_empty_and_garbage() {
    assert!(parse_list("[]").unwrap().is_empty());
    assert!(parse_list("not json").is_err());
}

#[test]
fn acquire_runs_warmup_and_builds_handle() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    let got = c.acquire(&req("sprites", Some("small"))).unwrap();
    assert_eq!(got.handle.as_str(), "sprites/moor-7-ab12cd");
    assert_eq!(got.workdir.as_str(), "/root/crabbox/cbx_1/moor-7-ab12cd");
    let calls = p.calls();
    let args = &calls.first().unwrap().args;
    assert_eq!(
        args,
        &[
            "warmup",
            "--provider",
            "sprites",
            "--slug",
            "moor-7-ab12cd",
            "--class",
            "standard"
        ]
    );
    assert_eq!(c.planned_handle(&req("sprites", None)), got.handle);
}

#[test]
fn acquire_maps_sizes_and_rejects_unknown() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    c.acquire(&req("sprites", Some("large"))).unwrap();
    let calls = p.calls();
    let first = calls.first().unwrap();
    assert!(first.args.contains(&"large".to_owned()));
    assert!(c.acquire(&req("sprites", Some("huge"))).is_err());
}

#[test]
fn ssh_calls_carry_static_env() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    c.acquire(&req("ssh", None)).unwrap();
    let handle = BoxHandle::new("ssh/moor-7-ab12cd");
    c.release(&handle).unwrap();
    for call in p.calls() {
        assert!(
            call.env
                .contains(&("CRABBOX_STATIC_ID".into(), "moor-7-ab12cd".into())),
            "{call:?}"
        );
        let port = ("CRABBOX_STATIC_PORT".into(), "22".into());
        assert!(call.env.contains(&port));
    }
}

#[test]
fn list_matches_top_level_slug_and_lease_label() {
    let items = parse_list(r#"[{"slug":"a"},{"labels":{"lease":"b"}}]"#).unwrap();
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

fn ssh_calls(p: &ScriptedProc) -> usize {
    p.calls().iter().filter(|c| c.tool == Tool::Ssh).count()
}

fn acquire_with_ready(ready: &str) -> (Arc<ScriptedProc>, BoxPath) {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok(format!("leased cbx_1 slug=-\n{ready}\n")),
    );
    let got = c.acquire(&req("ssh", None)).unwrap();
    (p, got.workdir)
}

#[test]
fn acquire_uses_workdir_from_warmup() {
    let (p, dir) =
        acquire_with_ready("ready ssh=u@h:22 workroot=/root/crabbox workdir=/root/crabbox/w/app");
    assert_eq!(dir.as_str(), "/root/crabbox/w/app");
    assert_eq!(ssh_calls(&p), 0);
}

#[test]
fn acquire_derives_the_workdir_from_the_workroot_like_crabbox() {
    let (p, dir) = acquire_with_ready("ready ssh=u@h:22 workroot=/r/ workdir=app");
    assert_eq!(dir.as_str(), "/r/cbx_1/moor-7-ab12cd");
    assert_eq!(ssh_calls(&p), 0);
}

#[test]
fn acquire_fails_closed_when_warmup_names_no_usable_dir() {
    for ready in ["ready ssh=u@h:22 workdir=app", "ready ssh=u@h:22 workdir="] {
        let (p, c) = fixture();
        p.on(
            Tool::Crabbox,
            &["warmup"],
            Reply::Ok(format!("leased cbx_1 slug=-\n{ready}\n")),
        );
        let err = c.acquire(&req("ssh", None)).unwrap_err();
        assert!(
            err.to_string().contains("neither a workdir nor a workroot"),
            "{ready}: {err:?}"
        );
        assert_eq!(ssh_calls(&p), 0, "{ready}");
    }
}

#[test]
fn acquire_unreadable_output_fails() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok("hello\n".into()));
    let err = c.acquire(&req("ssh", None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}

#[test]
fn acquire_failure_maps_through_errors() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], fail(1, "boom"));
    assert!(c.acquire(&req("ssh", None)).is_err());
}

fn stops(p: &ScriptedProc) -> Vec<Vec<String>> {
    p.calls()
        .into_iter()
        .map(|call| call.args)
        .filter(|args| args.first().is_some_and(|a| a == "stop"))
        .collect()
}

#[test]
fn timed_out_first_acquire_deletes_its_lease_then_drops_the_dir() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Timeout);
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    let r = req("sprites", None);
    let dir = c.lease_dir(&c.planned_handle(&r)).unwrap();
    let err = c.acquire(&r).unwrap_err();
    assert!(matches!(err, DriverError::Timeout(_)), "{err:?}");
    let want = [
        "stop",
        "--provider",
        "sprites",
        "--id",
        "moor-7-ab12cd",
        "--delete",
    ];
    assert_eq!(stops(&p), [want.map(str::to_owned).to_vec()]);
    assert!(!dir.exists(), "lease dir outlived the release");
}

#[test]
fn failed_release_after_failed_acquire_keeps_the_dir_and_names_both() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Timeout);
    p.on(
        Tool::Crabbox,
        &["stop"],
        fail(1, "provider refused the delete"),
    );
    let r = req("sprites", None);
    let dir = c.lease_dir(&c.planned_handle(&r)).unwrap();
    let msg = c.acquire(&r).unwrap_err().to_string();
    assert!(msg.contains("timed out"), "{msg}");
    assert!(msg.contains("provider refused the delete"), "{msg}");
    assert_eq!(stops(&p).len(), 1, "no retry of the release");
    assert!(
        dir.exists(),
        "lease dir dropped though its lease may still run"
    );
}

#[test]
fn failed_acquire_of_an_existing_lease_keeps_it() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Timeout);
    let r = req("sprites", None);
    let dir = c.lease_dir(&c.planned_handle(&r)).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    assert!(c.acquire(&r).is_err());
    assert!(stops(&p).is_empty());
    assert!(dir.exists());
}

#[test]
fn release_stops_by_provider_and_name() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    let got = c.release(&BoxHandle::new("sprites/moor-t7")).unwrap();
    assert_eq!(got, Released::Deleted);
    let calls = p.calls();
    let call = calls.first().unwrap();
    assert_eq!(
        call.args,
        [
            "stop",
            "--provider",
            "sprites",
            "--id",
            "moor-t7",
            "--delete"
        ]
    );
    assert!(!call.args.contains(&"--json".to_owned()));
}

#[test]
fn release_not_found_is_already_gone() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["stop"], fail(4, "lease not found\n"));
    let got = c.release(&BoxHandle::new("sprites/moor-t7")).unwrap();
    assert_eq!(got, Released::AlreadyGone);
}

#[test]
fn release_bare_name_omits_provider() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    c.release(&BoxHandle::new("moor-t7")).unwrap();
    let calls = p.calls();
    assert_eq!(
        calls.first().unwrap().args,
        ["stop", "--id", "moor-t7", "--delete"]
    );
}

#[test]
fn release_other_failure_is_error() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["stop"], fail(1, "boom"));
    assert!(c.release(&BoxHandle::new("sprites/x")).is_err());
}

#[test]
fn list_maps_items_to_handles() {
    let (p, c) = fixture();
    let json = r#"[{"id":1,"name":"a","status":"running","labels":{"slug":"moor-t1"}},
        {"id":2,"name":"b","status":"off","provider":"sprites"}]"#;
    p.on(Tool::Crabbox, &["list"], Reply::Ok(json.to_owned()));
    let got = c.list(&ProviderId::new("sprites")).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got.first().unwrap().handle.as_str(), "sprites/moor-t1");
    let second = got.get(1).unwrap();
    assert_eq!(second.handle.as_str(), "sprites/b");
    assert!(second.asleep);
    assert_eq!(
        p.calls().first().unwrap().args,
        ["list", "--provider", "sprites", "--json"]
    );
}

#[test]
fn list_is_bounded_to_a_short_deadline() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["list"], Reply::Ok("[]".into()));
    c.list(&ProviderId::new("sprites")).unwrap();
    let deadline = p.calls().first().unwrap().deadline;
    assert!(deadline.0 <= Duration::from_secs(5));
}

#[test]
fn list_timeout_is_an_error() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["list"], Reply::Timeout);
    assert!(matches!(
        c.list(&ProviderId::new("sprites")),
        Err(DriverError::Timeout(_))
    ));
}

#[test]
fn list_bad_json_fails() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["list"], Reply::Ok("nope".into()));
    assert!(matches!(
        c.list(&ProviderId::new("sprites")),
        Err(DriverError::Failed { .. })
    ));
}

#[test]
fn list_of_static_provider_is_unsupported() {
    let (p, c) = fixture();
    let err = c.list(&ProviderId::new("ssh")).unwrap_err();
    assert!(matches!(err, DriverError::Unsupported(_)));
    assert!(p.calls().is_empty());
}

#[test]
fn lookup_deadline_defaults_by_provider() {
    let c = Crabbox::fake(Arc::new(ScriptedProc::new()));
    assert_eq!(c.lookup_deadline("aws"), Duration::from_secs(5));
    assert_eq!(c.lookup_deadline("external"), Duration::from_secs(20));
}

#[test]
fn lookup_deadline_override_wins() {
    let c = Crabbox::fake(Arc::new(ScriptedProc::new())).with_lookup_deadlines(
        [("aws", 9), ("external", 40)]
            .map(|(k, s)| (k.to_owned(), Duration::from_secs(s)))
            .into(),
    );
    assert_eq!(c.lookup_deadline("aws"), Duration::from_secs(9));
    assert_eq!(c.lookup_deadline("external"), Duration::from_secs(40));
    assert_eq!(c.lookup_deadline("ssh"), Duration::from_secs(5));
}
