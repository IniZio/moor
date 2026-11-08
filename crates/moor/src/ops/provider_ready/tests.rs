//! Tests for `provider-ready`.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::{NodeConfig, UserConfig};
use moor_core::seam::proc::Tool;
use moor_core::view::node::ProviderCheck;

use super::run;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

const OUT: &str = concat!(
    "adapter: noise\n",
    r#"{"ok":false,"checks":["#,
    r#"{"status":"ok","check":"cli","message":"fine"},"#,
    r#"{"status":"failed","check":"provider","message":"SENTINEL-VALUE at /home/x","#,
    r#""details":{"hint":"set the key","fingerprint":"SENTINEL-VALUE"}}]}"#,
    "\n"
);

fn cx(dir: &std::path::Path, proc: Arc<ScriptedProc>, runtimes: &[&str]) -> Cx {
    let nodes = runtimes
        .iter()
        .enumerate()
        .map(|(n, rt)| {
            (
                format!("n{n}"),
                NodeConfig {
                    runtime: (*rt).to_owned(),
                    ..NodeConfig::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        proc,
    )
    .with_config(UserConfig {
        nodes,
        ..UserConfig::default()
    })
}

#[test]
fn only_the_check_name_status_and_hint_leave_the_controller() {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Crabbox,
        &["doctor", "--provider", "aws", "--json"],
        Reply::Fail {
            code: 1,
            stdout: OUT.to_owned(),
            stderr: String::new(),
        },
    );
    proc.on(
        Tool::Crabbox,
        &["doctor", "--provider", "static", "--json"],
        Reply::Ok(r#"{"ok":true,"checks":[]}"#.to_owned()),
    );
    let v = run(&cx(dir.path(), proc, &["static", "aws", "aws"])).unwrap();
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("SENTINEL-VALUE"), "{text}");
    let names: Vec<_> = v.providers.iter().map(|p| p.provider.as_str()).collect();
    assert_eq!(names, ["aws", "static"]);
    let [aws, st] = v.providers.as_slice() else {
        return assert_eq!(v.providers.len(), 2);
    };
    assert!(!aws.ok);
    assert_eq!(
        aws.failing,
        [ProviderCheck {
            check: "provider".to_owned(),
            status: "failed".to_owned(),
            class: None,
            error: None,
            hint: Some("set the key".to_owned()),
        }]
    );
    assert!(st.ok);
}

#[test]
fn a_crabbox_that_will_not_run_is_one_failing_check() {
    let dir = tempfile::tempdir().unwrap();
    let v = run(&cx(dir.path(), Arc::new(ScriptedProc::new()), &["aws"])).unwrap();
    let [p] = v.providers.as_slice() else {
        return assert_eq!(v.providers.len(), 1);
    };
    assert!(!p.ok);
    assert_eq!(
        p.failing,
        [ProviderCheck {
            check: "crabbox".to_owned(),
            status: "failed".to_owned(),
            class: None,
            error: None,
            hint: None,
        }]
    );
}

#[test]
fn crabbox_runs_in_an_empty_directory_under_the_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Crabbox,
        &["doctor", "--provider", "aws", "--json"],
        Reply::Ok(r#"{"ok":true,"checks":[]}"#.to_owned()),
    );
    run(&cx(dir.path(), Arc::clone(&proc), &["aws"])).unwrap();
    let calls = proc.calls();
    let cwd = calls.first().and_then(|c| c.cwd.clone()).expect("a cwd");
    assert!(cwd.starts_with(dir.path()), "{cwd:?}");
    assert!(cwd.is_dir());
    assert_eq!(std::fs::read_dir(&cwd).unwrap().count(), 0);
}

#[test]
fn only_a_short_error_class_is_kept_and_the_error_is_one_clean_line() {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    let out = concat!(
        r#"{"ok":false,"checks":["#,
        r#"{"status":"failed","check":"provider","message":"SENTINEL-VALUE","#,
        r#""details":{"class":"config","error":"first\nsecond\u001b[31m","hint":"fix it"}},"#,
        r#"{"status":"failed","check":"other","details":{"class":"has spaces SENTINEL-VALUE"}}]}"#
    );
    proc.on(
        Tool::Crabbox,
        &["doctor", "--provider", "aws", "--json"],
        Reply::Ok(out.to_owned()),
    );
    let v = run(&cx(dir.path(), proc, &["aws"])).unwrap();
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("has spaces"), "{text}");
    assert!(!text.contains("SENTINEL-VALUE"), "{text}");
    let [aws] = v.providers.as_slice() else {
        return assert_eq!(v.providers.len(), 1);
    };
    let errors: Vec<_> = aws.failing.iter().map(|c| c.error.as_deref()).collect();
    assert_eq!(errors, [Some("first second[31m"), None]);
    let classes: Vec<_> = aws.failing.iter().map(|c| c.class.as_deref()).collect();
    assert_eq!(classes, [Some("config"), None]);
}

#[test]
fn a_long_error_is_cut_to_one_bounded_line() {
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    let out = format!(
        r#"{{"ok":false,"checks":[{{"status":"failed","check":"provider","details":{{"error":"{}"}}}}]}}"#,
        "x".repeat(500)
    );
    proc.on(
        Tool::Crabbox,
        &["doctor", "--provider", "aws", "--json"],
        Reply::Ok(out),
    );
    let v = run(&cx(dir.path(), proc, &["aws"])).unwrap();
    let err = v
        .providers
        .first()
        .and_then(|p| p.failing.first())
        .and_then(|c| c.error.clone())
        .expect("an error");
    assert_eq!(err.chars().count(), 200, "{err}");
    assert!(err.ends_with('…'));
}
