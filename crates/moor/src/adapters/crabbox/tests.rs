//! Tests for the crabbox adapter.

use std::path::Path;
use std::sync::Arc;

use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::name::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};
use moor_core::seam::driver::{
    AcquireReq, Driver, DriverError, EgressApplied, EgressPlan, ExecOutcome, ExecSpec, Released,
};
use moor_core::seam::proc::{Deadline, Output, Proc, ProcError, ProcOutput, ProcSpec, Tool};
use zeroize::Zeroizing;

use super::parse::{parse_list, parse_warmup};
use super::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

mod workdir;

const PROBE_DIR: &str = "/root/crabbox/cbx_1/app";

fn probe(p: &ScriptedProc) {
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok(format!("MOOR_WORKDIR={PROBE_DIR}\n")),
    );
}

const WARMUP_OUT: &str = "leased cbx_1 slug=moor-t7 provider=sprites server=1 type=t ip=1.2.3.4\n\
ready ssh=u@h:22 network=public workroot=/root/crabbox\n";

fn req(provider: &str, egress: EgressPlan, class: Option<&str>) -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new(provider),
        task: TaskId::new(7),
        name: TaskName::parse("t7").unwrap(),
        egress,
        class: class.map(str::to_owned),
        nonce: "ab12cd".to_owned(),
        worktree: None,
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
    probe(&p);
    let got = c
        .acquire(&req("sprites", EgressPlan::Open, Some("small")))
        .unwrap();
    assert_eq!(got.handle.as_str(), "sprites/moor-t7-ab12cd");
    assert_eq!(got.workdir.as_str(), PROBE_DIR);
    let calls = p.calls();
    let args = &calls.first().unwrap().args;
    assert_eq!(
        args,
        &[
            "warmup",
            "--provider",
            "sprites",
            "--slug",
            "moor-t7-ab12cd",
            "--class",
            "standard"
        ]
    );
    assert!(calls
        .first()
        .unwrap()
        .env
        .contains(&("CRABBOX_NAMESPACE_SIZE".into(), "S".into())));
    assert_eq!(
        c.planned_handle(&req("sprites", EgressPlan::Open, None)),
        got.handle
    );
}

#[test]
fn open_plan_reports_open_egress() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let got = c.acquire(&req("sprites", EgressPlan::Open, None)).unwrap();
    assert_eq!(got.egress, EgressApplied::Open);
}

#[test]
fn acquire_maps_sizes_and_rejects_unknown() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    c.acquire(&req("sprites", EgressPlan::Open, Some("large")))
        .unwrap();
    let calls = p.calls();
    let first = calls.first().unwrap();
    assert!(first.args.contains(&"large".to_owned()));
    assert!(first
        .env
        .contains(&("CRABBOX_NAMESPACE_SIZE".into(), "L".into())));
    assert!(c
        .acquire(&req("sprites", EgressPlan::Open, Some("huge")))
        .is_err());
}

#[test]
fn ssh_calls_carry_static_env() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    probe(&p);
    c.acquire(&req("ssh", EgressPlan::Open, None)).unwrap();
    c.release(&BoxHandle::new("ssh/moor-t7-ab12cd")).unwrap();
    for call in p.calls() {
        assert!(
            call.env
                .contains(&("CRABBOX_STATIC_ID".into(), "moor-t7-ab12cd".into())),
            "{call:?}"
        );
        assert!(call
            .env
            .contains(&("CRABBOX_STATIC_PORT".into(), "22".into())));
    }
}

#[test]
fn list_matches_top_level_slug_and_lease_label() {
    let items = parse_list(r#"[{"slug":"a"},{"labels":{"lease":"b"}}]"#).unwrap();
    let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[test]
fn acquire_sprites_allow_list_uses_flag() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let plan = EgressPlan::Allow(vec!["a.com".into(), "b.com".into()]);
    let got = c.acquire(&req("sprites", plan, None)).unwrap();
    assert_eq!(got.egress, EgressApplied::Enforced);
    let calls = p.calls();
    let args = &calls.first().unwrap().args;
    assert_eq!(args.last().unwrap(), "a.com,b.com");
    assert!(args.contains(&"--sprites-network-allow".to_owned()));
}

#[test]
fn acquire_namespace_allow_list_uses_env() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let plan = EgressPlan::Allow(vec!["a.com".into()]);
    c.acquire(&req("namespace-devbox", plan, None)).unwrap();
    let calls = p.calls();
    let env = &calls.first().unwrap().env;
    assert!(env.contains(&("CRABBOX_NAMESPACE_EGRESS_DOMAINS".into(), "a.com".into())));
}

#[test]
fn acquire_unsupported_provider_reports_not_enforced() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let plan = EgressPlan::Allow(vec!["a.com".into()]);
    let got = c.acquire(&req("ssh", plan, None)).unwrap();
    assert_eq!(got.egress, EgressApplied::NotEnforced);
}

#[test]
fn acquire_without_workroot_still_gets_workdir_from_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\n".into()),
    );
    probe(&p);
    let got = c.acquire(&req("ssh", EgressPlan::Open, None)).unwrap();
    assert_eq!(got.workdir.as_str(), PROBE_DIR);
    assert!(p
        .calls()
        .iter()
        .all(|call| call.args.first().map(String::as_str) != Some("inspect")));
}

#[test]
fn acquire_unreadable_output_fails() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok("hello\n".into()));
    let err = c.acquire(&req("ssh", EgressPlan::Open, None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
}

#[test]
fn acquire_runs_warmup_in_worktree() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    probe(&p);
    let mut r = req("sprites", EgressPlan::Open, None);
    r.worktree = Some("/wt".into());
    c.acquire(&r).unwrap();
    let calls = p.calls();
    let first = calls.first().expect("one call recorded");
    assert_eq!(first.cwd.as_deref(), Some(std::path::Path::new("/wt")));
}

#[test]
fn acquire_failure_maps_through_errors() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["warmup"], fail(1, "boom"));
    assert!(c.acquire(&req("ssh", EgressPlan::Open, None)).is_err());
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
        ["stop", "--provider", "sprites", "--id", "moor-t7"]
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
    assert_eq!(calls.first().unwrap().args, ["stop", "--id", "moor-t7"]);
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
fn workdir_runs_the_pwd_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("noise\nMOOR_WORKDIR=/w/cbx_9f/app\n".into()),
    );
    let got = c
        .workdir(&BoxHandle::new("ssh/moor-t7"), Path::new("/wt/app"))
        .unwrap();
    assert_eq!(got.as_str(), "/w/cbx_9f/app");
    let calls = p.calls();
    let first = calls.first().unwrap();
    assert_eq!(
        first.args,
        [
            "run",
            "--provider",
            "ssh",
            "--id",
            "moor-t7",
            "--no-hydrate",
            "--no-sync",
            "--",
            "sh",
            "-c",
            r#"printf "MOOR_WORKDIR=%s\n" "$(pwd)""#
        ]
    );
    assert_eq!(first.cwd.as_deref(), Some(Path::new("/wt/app")));
}

#[test]
fn workdir_missing_box_is_not_found() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], fail(4, "not found"));
    let err = c
        .workdir(&BoxHandle::new("ssh/x"), Path::new("/wt/app"))
        .unwrap_err();
    assert_eq!(err, DriverError::NotFound);
}

#[test]
fn caps_asks_providers_json() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["providers"], Reply::Ok("[]".into()));
    let _ = c.caps(&ProviderId::new("sprites"));
    assert_eq!(p.calls().first().unwrap().args, ["providers", "--json"]);
}

#[test]
fn list_of_static_provider_is_unsupported() {
    let (p, c) = fixture();
    let err = c.list(&ProviderId::new("ssh")).unwrap_err();
    assert!(matches!(err, DriverError::Unsupported(_)));
    assert!(p.calls().is_empty());
}

const EXEC_DEADLINE: Deadline = Deadline(Duration::from_secs(90));

fn exec_spec(argv: &[&str]) -> ExecSpec {
    ExecSpec {
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: None,
        class: None,
    }
}

fn exec_on(c: &Crabbox, spec: &ExecSpec) -> Result<ExecOutcome, DriverError> {
    c.exec(
        &BoxHandle::new("sprites/moor-t7-ab12cd"),
        spec,
        EXEC_DEADLINE,
    )
}

/// A spawner that never starts anything.
struct NoSpawn;

impl Proc for NoSpawn {
    fn run(&self, _: &ProcSpec) -> Result<ProcOutput, ProcError> {
        Err(ProcError::Spawn(std::io::Error::other("missing")))
    }
    fn run_secret(&self, _: &ProcSpec) -> Result<Zeroizing<Vec<u8>>, ProcError> {
        Err(ProcError::Spawn(std::io::Error::other("missing")))
    }
    fn run_streamed(&self, _: &ProcSpec, _: &Output) -> Result<(), ProcError> {
        Err(ProcError::Spawn(std::io::Error::other("missing")))
    }
    fn spawn_detached(&self, _: &ProcSpec) -> Result<(), ProcError> {
        Err(ProcError::Spawn(std::io::Error::other("missing")))
    }
    fn alive(&self, _: u32) -> bool {
        false
    }
}

#[test]
fn exec_runs_without_sync_on_the_target() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], Reply::Ok(String::new()));
    let out = exec_on(&c, &exec_spec(&["make", "test"])).unwrap();
    assert_eq!(out, ExecOutcome::Exited { code: 0 });
    let calls = p.calls();
    let call = calls.first().unwrap();
    assert_eq!(
        call.args,
        [
            "run",
            "--provider",
            "sprites",
            "--id",
            "moor-t7-ab12cd",
            "--no-hydrate",
            "--no-sync",
            "--",
            "sh",
            "-c",
            super::exec::UNSET_THEN_EXEC,
            "sh",
            "make",
            "test"
        ]
    );
    assert!(call.cwd.is_none());
}

#[test]
fn exec_carries_env_by_allow_list_not_argv() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], Reply::Ok(String::new()));
    let mut spec = exec_spec(&["env"]);
    spec.env.push(("TOKEN".to_owned(), "s3cret".to_owned()));
    exec_on(&c, &spec).unwrap();
    let calls = p.calls();
    let call = calls.first().unwrap();
    let pos = call.args.iter().position(|a| a == "--allow-env").unwrap();
    assert_eq!(call.args.get(pos + 1).map(String::as_str), Some("TOKEN"));
    assert!(pos < call.args.iter().position(|a| a == "--").unwrap());
    assert!(!call.args.iter().any(|a| a.contains("s3cret")));
    assert!(call.env.contains(&("TOKEN".into(), "s3cret".into())));
}

#[test]
fn exec_cwd_wraps_argv_in_a_cd() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], Reply::Ok(String::new()));
    let mut spec = exec_spec(&["./install.sh", "-q"]);
    spec.cwd = Some(BoxPath::under("/root/crabbox", "moor-t7-ab12cd").unwrap());
    exec_on(&c, &spec).unwrap();
    let calls = p.calls();
    let args = &calls.first().unwrap().args;
    let sep = args.iter().position(|a| a == "--").unwrap();
    let mut tail = args.iter().skip(sep + 1).map(String::as_str);
    assert_eq!(tail.next(), Some("sh"));
    assert_eq!(tail.next(), Some("-c"));
    assert!(tail
        .next()
        .is_some_and(|script| script.contains("cd \"$1\"")));
    let rest: Vec<_> = tail.collect();
    assert_eq!(
        rest,
        ["sh", "/root/crabbox/moor-t7-ab12cd", "./install.sh", "-q"]
    );
}

#[test]
fn exec_ssh_carries_static_env() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], Reply::Ok(String::new()));
    let h = BoxHandle::new("ssh/box1");
    c.exec(&h, &exec_spec(&["true"]), EXEC_DEADLINE).unwrap();
    let calls = p.calls();
    let env = &calls.first().unwrap().env;
    assert!(env.contains(&("CRABBOX_STATIC_ID".into(), "box1".into())));
}

#[test]
fn exec_remote_failure_is_an_outcome() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["run"],
        fail(17, "boom\nremote command exited 17\n"),
    );
    let out = exec_on(&c, &exec_spec(&["false"])).unwrap();
    assert_eq!(out, ExecOutcome::Exited { code: 17 });
}

#[test]
fn exec_other_failure_maps_through_errors() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["run"],
        fail(2, "unauthorized: invalid token"),
    );
    assert!(matches!(
        exec_on(&c, &exec_spec(&["true"])),
        Err(DriverError::Auth(_))
    ));
}

#[test]
fn exec_mismatched_exit_code_is_not_remote() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], fail(2, "remote command exited 9"));
    assert!(exec_on(&c, &exec_spec(&["true"])).is_err());
}

#[test]
fn exec_remote_exit_code_must_match_whole() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], fail(1, "remote command exited 17"));
    assert!(exec_on(&c, &exec_spec(&["true"])).is_err());
}

#[test]
fn exec_timeout_reports_the_deadline() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["run"], Reply::Timeout);
    assert_eq!(
        exec_on(&c, &exec_spec(&["sleep", "9"])),
        Err(DriverError::Timeout(EXEC_DEADLINE.0))
    );
}

#[test]
fn exec_spawn_failure_is_unavailable() {
    let c = Crabbox::fake(Arc::new(NoSpawn));
    assert!(matches!(
        exec_on(&c, &exec_spec(&["true"])),
        Err(DriverError::Unavailable(_))
    ));
}
