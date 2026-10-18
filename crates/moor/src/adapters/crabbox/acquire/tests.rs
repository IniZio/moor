//! Tests for acquiring a Box with a repo-declared shape.

use std::sync::Arc;

use moor_core::config::BoxShape;
use moor_core::name::{ProviderId, TaskId, TaskName};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

const WARMUP_OUT: &str = "leased cbx_1 slug=moor-t7 provider=external server=1 type=t ip=1.2.3.4\n\
ready ssh=u@h:22 network=public workroot=/root/crabbox\n";

fn req(provider: &str, shape: BoxShape) -> AcquireReq {
    AcquireReq {
        provider: ProviderId::new(provider),
        task: TaskId::new(7),
        name: TaskName::parse("t7").unwrap(),
        class: None,
        nonce: "ab12cd".to_owned(),
        worktree: None,
        shape,
    }
}

fn shape() -> BoxShape {
    BoxShape {
        template: Some("t".to_owned()),
        disk_gb: Some(20),
    }
}

fn scripted() -> Arc<ScriptedProc> {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["warmup"], Reply::Ok(WARMUP_OUT.to_owned()));
    p.on(
        Tool::Crabbox,
        &["run"],
        Reply::Ok("MOOR_WORKDIR=/root/crabbox/cbx_1/app\n".to_owned()),
    );
    p
}

const BASE_ARGV: [&str; 7] = [
    "warmup",
    "--provider",
    "external",
    "--slug",
    "moor-t7-ab12cd",
    "--class",
    "standard",
];

fn warmup_call(p: &ScriptedProc) -> crate::testing::proc::Call {
    let calls = p.calls();
    let call = calls
        .iter()
        .find(|c| c.args.first().is_some_and(|a| a == "warmup"));
    call.expect("warmup call").clone()
}

fn warmup_args(p: &ScriptedProc) -> Vec<String> {
    warmup_call(p).args
}

fn warmup_env(p: &ScriptedProc) -> Vec<(String, String)> {
    warmup_call(p).env
}

#[test]
fn external_shape_sets_env_and_keeps_the_argv() {
    let p = scripted();
    let c = Crabbox::fake(p.clone());
    c.acquire(&req("external", shape())).unwrap();
    assert_eq!(warmup_args(&p), BASE_ARGV);
    let env = warmup_env(&p);
    assert!(env.contains(&("CRABBOX_AGENT37_TEMPLATE".to_owned(), "t".to_owned())));
    assert!(env.contains(&("CRABBOX_AGENT37_DISK_GB".to_owned(), "20".to_owned())));
}

#[test]
fn unset_shape_keys_send_no_env() {
    let p = scripted();
    let c = Crabbox::fake(p.clone());
    let only_disk = BoxShape {
        template: None,
        disk_gb: Some(9),
    };
    c.acquire(&req("external", only_disk)).unwrap();
    let env = warmup_env(&p);
    assert!(env.iter().all(|(k, _)| k != "CRABBOX_AGENT37_TEMPLATE"));
    assert!(env.contains(&("CRABBOX_AGENT37_DISK_GB".to_owned(), "9".to_owned())));
    let p = scripted();
    let c = Crabbox::fake(p.clone());
    c.acquire(&req("external", BoxShape::default())).unwrap();
    assert!(warmup_env(&p)
        .iter()
        .all(|(k, _)| !k.starts_with("CRABBOX_AGENT37_")));
}

#[test]
fn empty_shape_keeps_the_old_argv() {
    let p = scripted();
    let c = Crabbox::fake(p.clone());
    c.acquire(&req("external", BoxShape::default())).unwrap();
    assert_eq!(
        warmup_args(&p),
        [
            "warmup",
            "--provider",
            "external",
            "--slug",
            "moor-t7-ab12cd",
            "--class",
            "standard"
        ]
    );
}

#[test]
fn shape_on_another_provider_is_refused_before_warmup() {
    let p = scripted();
    let c = Crabbox::fake(p.clone());
    let err = c.acquire(&req("aws", shape())).unwrap_err();
    assert!(matches!(err, DriverError::Failed { .. }), "{err:?}");
    let summary = err.to_string();
    assert!(summary.contains("box.template, box.disk_gb"), "{summary}");
    assert!(summary.contains("`aws`"), "{summary}");
    assert!(p.calls().is_empty(), "{:?}", p.calls());
}

#[test]
fn refusal_names_only_the_keys_set() {
    let c = Crabbox::fake(scripted());
    let only_disk = BoxShape {
        template: None,
        disk_gb: Some(9),
    };
    let err = c.acquire(&req("aws", only_disk)).unwrap_err().to_string();
    assert!(err.contains("box.disk_gb"), "{err}");
    assert!(!err.contains("box.template"), "{err}");
}

#[test]
fn adapter_rejection_text_reaches_the_caller() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Crabbox,
        &["warmup"],
        Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: "agent37: diskGb 99 is above 20 for class fast".to_owned(),
        },
    );
    let c = Crabbox::fake(p);
    let err = c
        .acquire(&req("external", shape()))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("agent37: diskGb 99 is above 20 for class fast"),
        "{err}"
    );
}
