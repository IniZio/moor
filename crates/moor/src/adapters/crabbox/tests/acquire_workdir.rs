//! Tests for which working directory acquire reports.

use std::sync::Arc;

use moor_core::name::BoxPath;
use moor_core::seam::driver::Driver;
use moor_core::seam::proc::Tool;

use super::{fail, fixture, probe, req, Reply, ScriptedProc, PROBE_DIR};

#[test]
fn acquire_without_workroot_still_gets_workdir_from_probe() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\n".into()),
    );
    probe(&p);
    let got = c.acquire(&req("ssh", None)).unwrap();
    assert_eq!(got.workdir.as_str(), PROBE_DIR);
    assert!(p
        .calls()
        .iter()
        .all(|call| call.args.first().map(String::as_str) != Some("inspect")));
}

fn run_calls(p: &ScriptedProc) -> usize {
    p.calls()
        .iter()
        .filter(|c| c.args.first().map(String::as_str) == Some("run"))
        .count()
}

fn acquire_with_ready(ready: &str) -> (Arc<ScriptedProc>, BoxPath) {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(r#"[{"provider":"hetzner"}]"#.into()),
    );
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok(format!("leased cbx_1 slug=-\n{ready}\n")),
    );
    probe(&p);
    let got = c.acquire(&req("hetzner", None)).unwrap();
    (p, got.workdir)
}

#[test]
fn acquire_uses_workdir_from_warmup_without_probing() {
    let (p, dir) =
        acquire_with_ready("ready ssh=u@h:22 workroot=/root/crabbox workdir=/root/crabbox/w/app");
    assert_eq!(dir.as_str(), "/root/crabbox/w/app");
    assert_eq!(run_calls(&p), 0);
}

#[test]
fn acquire_trusts_the_warmup_workdir_when_the_provider_lookup_fails() {
    let (p, c) = fixture();
    p.on(Tool::Crabbox, &["providers"], fail(1, "boom"));
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\nready ssh=u@h:22 workdir=/root/w/app\n".into()),
    );
    probe(&p);
    let got = c.acquire(&req("hetzner", None)).unwrap();
    assert_eq!(got.workdir.as_str(), "/root/w/app");
    assert_eq!(got.platform.os, "Linux");
    assert_eq!(run_calls(&p), 0);
}

#[test]
fn acquire_on_a_static_host_probes_even_with_a_warmup_workdir() {
    let (p, c) = fixture();
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(r#"[{"provider":"ssh","aliases":["static"]}]"#.into()),
    );
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Ok("leased cbx_1 slug=-\nready ssh=u@h:22 workdir=/Users/me/w/app\n".into()),
    );
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/Users/me/crabbox/app\nMOOR_UNAME=Darwin arm64\n".into()),
    );
    let got = c.acquire(&req("ssh", None)).unwrap();
    assert_eq!(got.workdir.as_str(), "/Users/me/w/app");
    assert_eq!(got.platform.os, "Darwin");
    assert_eq!(run_calls(&p), 1);
}

#[test]
fn acquire_probes_when_warmup_workdir_is_relative_or_empty() {
    for ready in [
        "ready ssh=u@h:22 workdir=app",
        "ready ssh=u@h:22 workdir=",
        "ready ssh=u@h:22 workroot=/r",
    ] {
        let (p, dir) = acquire_with_ready(ready);
        assert_eq!(dir.as_str(), PROBE_DIR, "{ready}");
        assert_eq!(run_calls(&p), 1, "{ready}");
    }
}
