//! Tests that a Box's workdir comes only from what crabbox reports at acquire.

use std::path::Path;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Driver, DriverError, ExecSpec};
use moor_core::seam::proc::{Deadline, Tool};

use super::{fixture, req, Reply};

#[test]
fn acquire_without_a_ready_line_fails_closed_and_never_searches_the_box() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_X slug=moor-t7-ab12cd\n".to_owned()),
    );
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("ssh -o StrictHostKeyChecking=yes u@h\n".to_owned()),
    );
    p.on(Tool::Ssh, &[], Reply::Ok("R 0 0\n\n".to_owned()));
    let err = c.acquire(&req("sprites", None)).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    let calls = p.calls();
    assert_eq!(calls.len(), 1, "the warmup only: {calls:?}");
}

#[test]
fn a_workdir_lookup_without_a_recorded_one_fails_closed_and_runs_nothing() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["ssh"],
        Reply::Ok("ssh -o StrictHostKeyChecking=yes u@h\n".to_owned()),
    );
    p.on(Tool::Ssh, &[], Reply::Ok("R 0 0\n\n".to_owned()));
    let err = c
        .workdir(&BoxHandle::new("sprites/moor-t7-ab12cd"))
        .unwrap_err();
    assert!(err.to_string().contains("no recorded workdir"), "{err:?}");
    assert!(p.calls().is_empty(), "{:?}", p.calls());
}

#[test]
fn a_relative_work_root_in_the_ready_line_is_refused_by_name() {
    for ready in [
        "ready ssh=u@h:22 workroot=moor",
        "ready ssh=u@h:22 workroot=moor workdir=moor/cbx_1/t7",
    ] {
        let (p, c) = fixture();
        p.on(
            Tool::Crabbox,
            &["warmup"],
            Reply::Ok(format!("leased cbx_1 slug=-\n{ready}\n")),
        );
        let err = c.acquire(&req("sprites", None)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("relative work root"), "{text}");
        assert!(text.contains("moor"), "{text}");
    }
}

#[test]
fn every_crabbox_call_asks_for_the_relative_work_root() {
    let (p, c) = fixture();
    let ok = |s: &str| Reply::Ok(s.to_owned());
    p.on(
        Tool::Crabbox,
        &["providers"],
        ok(r#"[{"provider":"sprites","features":["pause-resume"]}]"#),
    );
    p.on(
        Tool::Crabbox,
        &["warmup"],
        ok("leased cbx_1 slug=-\nready ssh=u@h:22 workroot=/root/moor\n"),
    );
    p.on(Tool::Crabbox, &["pause"], ok(""));
    p.on(Tool::Crabbox, &["resume"], ok(""));
    p.on(Tool::Crabbox, &["stop"], ok(""));
    p.on(
        Tool::Crabbox,
        &["ssh"],
        ok("ssh -o StrictHostKeyChecking=yes u@h\n"),
    );
    p.on(Tool::Ssh, &[], ok("R 0 0\n\n"));
    p.on(Tool::Crabbox, &["heartbeat"], ok(""));
    p.on_child(
        Tool::Crabbox,
        &["tunnel"],
        crate::testing::proc::ChildReply::Ready(r#"{"box":8080,"socket":"/tmp/px"}"#.to_owned()),
    );
    let handle = BoxHandle::new("sprites/moor-t7-ab12cd");
    c.acquire(&req("sprites", None)).unwrap();
    c.sleep(&handle).unwrap();
    c.wake(&handle).unwrap();
    let _ = c.exec(
        &handle,
        &ExecSpec {
            argv: vec!["true".to_owned()],
            env: Vec::new(),
            cwd: None,
            output: None,
        },
        Deadline(Duration::from_secs(5)),
    );
    let _tunnel = c
        .reverse_tunnel(&handle, 8080, Path::new("/tmp/px"))
        .unwrap();
    c.release(&handle).unwrap();
    let verbs: Vec<_> = p
        .calls()
        .iter()
        .filter_map(|c| c.args.first().cloned())
        .collect();
    for verb in ["warmup", "pause", "resume", "ssh", "tunnel", "stop"] {
        assert!(
            verbs.iter().any(|v| v == verb),
            "{verb} not called: {verbs:?}"
        );
    }
    for call in p.calls().iter().filter(|c| c.tool == Tool::Crabbox) {
        let root = ("CRABBOX_EXTERNAL_WORK_ROOT".to_owned(), "moor".to_owned());
        let leaked = call.env.iter().any(|(k, _)| k == "CRABBOX_WORK_ROOT");
        assert!(call.env.contains(&root) && !leaked, "{call:?}");
    }
}

/// A plan for repo `o/r` built from `repo_yaml`.
fn plan(repo_yaml: &str) -> moor_core::cache::CachePlan {
    let layers = moor_core::cache::CacheLayers {
        repo: moor_core::config::parse_repo(".moor.yaml", repo_yaml)
            .unwrap()
            .cache,
        operator: moor_core::cache::CacheConfig::default(),
        ..moor_core::cache::CacheLayers::default()
    };
    let repo = moor_core::name::ForgeRepo {
        owner: "o".to_owned(),
        name: "r".to_owned(),
    };
    let salt = moor_core::cache::CacheSalt::new([3; 64]);
    let user = moor_core::user::UserId::new(1);
    moor_core::cache::CachePlan::build(user, Some(&repo), &layers, &salt).unwrap()
}

fn acquired_in(repo_yaml: &str) -> String {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok(
            "leased cbx_1 slug=-\nready ssh=u@h:22 workroot=/home/u/moor workdir=/home/u/moor/cbx_1/moor-7-ab12cd\n"
                .to_owned(),
        ),
    );
    let mut r = req("sprites", None);
    r.cache = plan(repo_yaml);
    c.acquire(&r).unwrap().workdir.as_str().to_owned()
}

#[test]
fn a_box_that_keeps_its_work_tree_works_in_the_repo_directory_under_the_work_root() {
    assert_eq!(acquired_in(""), "/home/u/moor/repos/r");
}

#[test]
fn a_box_without_the_work_tree_volume_keeps_the_per_lease_workdir() {
    assert_eq!(
        acquired_in("cache: {work_tree: false}"),
        "/home/u/moor/cbx_1/moor-7-ab12cd"
    );
}
