//! No resolved credential value crosses the controller socket or reaches the Box.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::parse_user;
use moor_core::seam::proc::Tool;
use moor_core::seam::proxy_host::ProxyHostError;
use moor_core::{Request, ShowReq, StartReq, SteerReq, TaskName};
use serde_json::{json, Value};

use super::serve;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::proxy_host::FakeProxyHost;
use crate::testing::surface::FakeSurface;
use crate::Moor;

/// What the configured credential command prints.
const SENTINEL: &str = "sk-sentinel-7f3a91c2d4e85b60";

/// A credential the Box is given, so `start` must resolve it.
const CONFIG: &str = "credentials:
  CLAUDE_CODE_OAUTH_TOKEN: {cmd: [echo, token]}
  MY_API_KEY: {cmd: [secret-tool, lookup, api]}
box_credentials: [MY_API_KEY]
";
struct World {
    driver: Arc<FakeDriver>,
    engine: Arc<FakeEngine>,
    proc: Arc<ScriptedProc>,
    surface: Arc<FakeSurface>,
    _dir: tempfile::TempDir,
}

fn world(host: FakeProxyHost) -> (Cx, World) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = Path::new("/repo");
    let proc = Arc::new(ScriptedProc::new());
    let git = |args: &[&str], out: String| {
        proc.on(Tool::Git, args, Reply::Ok(out));
    };
    git(&["rev-parse", "--show-toplevel"], "/repo\n".to_owned());
    git(&["ls-tree"], String::new());
    git(&["rev-parse", "HEAD"], "abc123\n".to_owned());
    git(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        "/repo/.git\n".to_owned(),
    );
    git(
        &["rev-parse", "--git-path", "info/exclude"],
        format!("{}/exclude\n", dir.path().display()),
    );
    proc.on(Tool::Sh, &["-c"], Reply::Ok(format!("{SENTINEL}\n")));
    let driver = Arc::new(FakeDriver::new());
    let surface = Arc::new(FakeSurface::new(dir.path()));
    let engine = Arc::new(FakeEngine::new());
    let cx = Cx::fake_with(
        dir.path(),
        repo,
        driver.clone(),
        surface.clone(),
        proc.clone(),
        parse_user("test", CONFIG).expect("config"),
    )
    .with_engine(engine.clone())
    .with_proxy_host(Arc::new(host));
    let world = World {
        driver,
        engine,
        proc,
        surface,
        _dir: dir,
    };
    (cx, world)
}

/// Serve hello then `requests` over `cx`; return every byte read and every byte written.
fn exchange(cx: Cx, requests: &[Request]) -> (String, String, Vec<Value>) {
    let moor = Moor::from_cx(cx);
    let mut lines = vec![json!({"frame": "hello", "protocol": 1})];
    for (i, r) in requests.iter().enumerate() {
        lines.push(json!({"frame": "request", "id": format!("r{i}"), "request": r}));
    }
    let input = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut out = Vec::new();
    serve(
        Cursor::new(input.clone().into_bytes()),
        &mut out,
        |r| moor.call(r),
        |_, _| {},
    );
    let out = String::from_utf8(out).expect("utf8");
    let frames = out
        .lines()
        .map(|l| serde_json::from_str(l).expect("json frame"))
        .collect();
    (input, out, frames)
}

fn start() -> Request {
    Request::Start(StartReq {
        name: TaskName::parse("demo").expect("name"),
        goal: Some("ship it".into()),
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        resume: false,
        trust_moor_yaml: false,
    })
}

fn show() -> Request {
    Request::Show(ShowReq {
        task: Some(TaskName::parse("demo").expect("name")),
        events: Some(100),
        agent: true,
        reports: true,
        pane: true,
    })
}

fn steer() -> Request {
    Request::Steer(SteerReq {
        task: TaskName::parse("demo").expect("name"),
        text: Some("continue".into()),
        menu: None,
    })
}

#[test]
fn start_show_steer_frames_and_the_box_never_carry_the_resolved_secret() {
    let (cx, w) = world(FakeProxyHost::new());
    let (input, out, frames) = exchange(cx, &[start(), show(), steer()]);
    assert_eq!(frames.len(), 4, "{out}");
    for f in &frames[1..] {
        assert_eq!(f["frame"], "response", "{out}");
        assert!(f["envelope"]["error"].is_null(), "{out}");
    }
    // The controller did resolve the credential, so the absence below is not vacuous.
    assert!(
        w.proc
            .calls()
            .iter()
            .any(|c| c.args.iter().any(|a| a == "moor-cred")),
        "credential command never ran"
    );
    assert!(!input.contains(SENTINEL), "client frames: {input}");
    assert!(!out.contains(SENTINEL), "server frames: {out}");

    assert_box_side_clean(&w);
}

#[test]
fn a_proxy_failure_log_carrying_the_nonce_does_not_reach_the_frames() {
    let nonce = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ";
    let log = format!("boom\nlistening for moor:{nonce}@127.0.0.1\nlast line");
    let host = FakeProxyHost::new().with_ensure_error(ProxyHostError::Failed(log));
    let (cx, w) = world(host);
    let (_, out, frames) = exchange(cx, &[start(), show()]);
    assert_eq!(frames.len(), 3, "{out}");
    assert!(w.engine.launches().is_empty(), "launch should have failed");
    assert!(out.contains("boom"), "failure should be reported: {out}");
    assert!(!out.contains(nonce), "server frames: {out}");
}

/// Everything the fake Box, agent engine and surface were handed.
fn assert_box_side_clean(w: &World) {
    assert!(!w.engine.launches().is_empty(), "agent never launched");
    let seen = [
        format!("{:?}", w.driver.execs()),
        format!("{:?}", w.driver.copies()),
        format!("{:?}", w.driver.calls()),
        format!("{:?}", w.engine.launches()),
        format!("{:?}", w.engine.steers()),
        format!("{:?}", w.surface.opened()),
    ];
    for text in seen {
        assert!(!text.contains(SENTINEL), "box side: {text}");
    }
}
