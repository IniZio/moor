//! Tests for `env` and its item assembly.

use std::path::Path;
use std::sync::Arc;

use moor_core::config::{parse_user, Layer, UserConfig};
use moor_core::seam::clock::Ts;
use moor_core::seam::proc::{Proc, Tool};
use moor_core::view::env::{EnvKind, EnvView};
use moor_core::{EnvReq, ErrorKind};

use super::run;
use crate::adapters::creds::Creds;
use crate::adapters::git::Git;
use crate::cx::{Cx, CxParts};
use crate::env::Env;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::rng::FakeRng;
use crate::testing::surface::FakeSurface;

/// Context whose repo root and main checkout are `top`, with user config `config`.
fn cx_with(top: &Path, config: UserConfig) -> Cx {
    cx_proc(top, config, ScriptedProc::new())
}

/// Like `cx_with`, over a caller-prepared scripted proc.
fn cx_proc(top: &Path, config: UserConfig, proc: ScriptedProc) -> Cx {
    proc.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        Reply::Ok(format!("{}\n", top.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", top.display())),
    );
    let proc: Arc<dyn Proc> = Arc::new(proc);
    Cx::new(CxParts {
        state_dir: Some(top.to_path_buf()),
        clock: Arc::new(FakeClock::new(Ts::from_millis(1000))),
        rng: Arc::new(FakeRng::default()),
        driver: Arc::new(FakeDriver::new()),
        surface: Arc::new(FakeSurface::new(top)),
        git: Git::new(Arc::clone(&proc)),
        proc: Arc::clone(&proc),
        config,
        creds: Creds::new(Env::default(), proc),
        cwd: top.to_path_buf(),
    })
}

/// Context with default user config.
fn cx(top: &Path) -> Cx {
    cx_with(top, UserConfig::default())
}

/// Run `env` in a repo holding `.moor.yaml` text `yaml`.
fn env_with(yaml: &str) -> (tempfile::TempDir, Cx, EnvView) {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(".moor.yaml"), yaml).unwrap();
    let cx = cx(d.path());
    let v = run(&cx, &EnvReq { repo: None }).expect("env ok");
    (d, cx, v)
}

fn kinds(v: &EnvView) -> Vec<EnvKind> {
    v.rows.iter().map(|i| i.kind).collect()
}

#[test]
fn outside_a_repo_fails() {
    let d = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        d.path(),
        d.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(d.path())),
        Arc::new(ScriptedProc::new()),
    );
    let err = run(&cx, &EnvReq { repo: None }).expect_err("fails");
    assert_eq!(err.kind, ErrorKind::Failed);
}

#[test]
fn item_order_is_fixed() {
    let (_d, _cx, v) = env_with("setup: [make]\negress: [example.com]\n");
    let k = kinds(&v);
    let pos = |kind| k.iter().position(|x| *x == kind).unwrap();
    assert!(pos(EnvKind::Setup) < pos(EnvKind::Egress));
    assert!(pos(EnvKind::Egress) < pos(EnvKind::Box));
    let boxes: Vec<_> = v
        .rows
        .iter()
        .filter(|i| i.kind == EnvKind::Box)
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(boxes, ["provider", "size", "gh"]);
}

#[test]
fn repo_only_credential_does_not_resolve_but_env_succeeds() {
    let (_d, _cx, v) = env_with("box_credentials: [TOKEN]\n");
    let cred = v
        .credentials
        .iter()
        .find(|c| c.which == "TOKEN")
        .expect("TOKEN is reported");
    assert!(!cred.ok);
    assert!(v.rows.iter().any(|r| r.name == "size"));
}

#[test]
fn repo_mcp_needs_approval() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join(".mcp.json"),
        r#"{"mcpServers":{"a":{},"b":{}}}"#,
    )
    .unwrap();
    std::fs::create_dir(d.path().join(".claude")).unwrap();
    std::fs::write(
        d.path().join(".claude/settings.local.json"),
        r#"{"enabledMcpjsonServers":["a"]}"#,
    )
    .unwrap();
    let v = run(&cx(d.path()), &EnvReq { repo: None }).unwrap();
    let bad: Vec<_> = v
        .credentials
        .iter()
        .filter(|c| c.kind == EnvKind::Mcp && !c.ok)
        .map(|c| c.which.as_str())
        .collect();
    assert_eq!(bad, ["b"]);
}

#[test]
fn invalid_mcp_json_is_one_failed_item() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(".mcp.json"), "{").unwrap();
    let v = run(&cx(d.path()), &EnvReq { repo: None }).unwrap();
    let m: Vec<_> = v
        .credentials
        .iter()
        .filter(|c| c.kind == EnvKind::Mcp)
        .collect();
    assert_eq!(m.len(), 1);
    assert!(m.first().is_some_and(|c| !c.ok));
}

/// Identity rows as `(name, value, source)`.
fn identity(v: &EnvView) -> Vec<(&str, &str, Layer)> {
    v.rows
        .iter()
        .filter(|r| r.kind == EnvKind::Identity)
        .map(|r| (r.name.as_str(), r.note.as_str(), r.source))
        .collect()
}

#[test]
fn identity_rows_come_from_git() {
    let d = tempfile::tempdir().unwrap();
    let proc = ScriptedProc::new();
    proc.on(
        Tool::Git,
        &["config", "--get", "user.name"],
        Reply::Ok("Ada\n".into()),
    );
    proc.on(
        Tool::Git,
        &["config", "--get", "user.email"],
        Reply::Ok("ada@example.com\n".into()),
    );
    let cx = cx_proc(d.path(), UserConfig::default(), proc);
    let v = run(&cx, &EnvReq { repo: None }).unwrap();
    assert_eq!(
        identity(&v),
        [
            ("user.name", "Ada", Layer::Git),
            ("user.email", "ada@example.com", Layer::Git)
        ]
    );
}

#[test]
fn missing_git_identity_adds_no_rows() {
    let d = tempfile::tempdir().unwrap();
    let proc = ScriptedProc::new();
    for key in ["user.name", "user.email"] {
        proc.on(
            Tool::Git,
            &["config", "--get", key],
            Reply::Fail {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
    }
    let cx = cx_proc(d.path(), UserConfig::default(), proc);
    let v = run(&cx, &EnvReq { repo: None }).unwrap();
    assert!(identity(&v).is_empty());
}

#[test]
fn github_block_without_app() {
    let (_d, _cx, v) = env_with("");
    assert_eq!(v.github.identity, "auto");
    assert!(v.github.app_id.is_none());
    assert_eq!(v.github.login, "logged_out");
    assert_eq!(v.github.mint.status, "skipped");
    assert_eq!(v.github.resolved.mode.as_deref(), Some("gh-you"));
}

#[test]
fn json_hosts_are_the_effective_allow_list() {
    let (_d, _cx, v) = env_with("egress: [example.com]\n");
    let j = serde_json::to_value(&v).unwrap();
    let hosts: Vec<&str> = j
        .pointer("/egress/hosts")
        .and_then(|h| h.as_array())
        .map(|a| a.iter().filter_map(|h| h.as_str()).collect())
        .unwrap_or_default();
    assert!(hosts.contains(&"github.com"), "{hosts:?}");
    assert_eq!(hosts.last(), Some(&"example.com"), "{hosts:?}");
}

#[test]
fn egress_note_lists_the_hosts_with_user_allow() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join(".moor.yaml"), "egress: [repo.example.com]\n").unwrap();
    let user = parse_user(
        "t",
        "box:\n  provider: sprites\negress:\n  allow: [user.example.com]\n",
    )
    .unwrap();
    let v = run(&cx_with(d.path(), user), &EnvReq { repo: None }).unwrap();
    assert!(v.egress.hosts.iter().any(|h| h == "user.example.com"));
    for h in &v.egress.hosts {
        assert!(
            v.egress.note.contains(h.as_str()),
            "{h} not in {:?}",
            v.egress.note
        );
    }
}
