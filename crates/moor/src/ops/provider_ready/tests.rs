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
            hint: None,
        }]
    );
}
