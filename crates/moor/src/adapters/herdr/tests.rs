//! Tests for the herdr Surface adapter.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::name::TaskName;
use moor_core::seam::proc::Tool;
use moor_core::seam::surface::{
    Key, PaneCmd, PaneRef, Surface, SurfaceError, WorkspaceRef, Worktree,
};

use super::{parse, Herdr};
use crate::testing::proc::{Call, Reply, ScriptedProc};

const WS_CREATE: &str = include_str!("testdata/workspace-create.json");
const PANE_LIST: &str = include_str!("testdata/pane-list.json");

const WT_CREATED: &str = r#"{"id":"cli:worktree:create","result":{"type":"worktree_created","workspace":{"workspace_id":"w2"},"worktree":{"path":"/r/wt","branch":"moor/a","open_workspace_id":"w2","extra":1}}}"#;
const WT_LIST: &str = r#"{"id":"cli:worktree:list","result":{"worktrees":[{"path":"/r/wt","branch":"moor/a","open_workspace_id":"w9"},{"path":"/r/other","branch":"x"}]}}"#;
const TAB_CREATED: &str = r#"{"id":"cli:tab:create","result":{"root_pane":{"pane_id":"w1:p2"},"tab":{"tab_id":"w1:t2"}}}"#;
const NOT_RUNNING: &str = r#"{"error":{"code":"server_not_running","message":"down"}}"#;

fn ok(s: &str) -> Reply {
    Reply::Ok(s.to_owned())
}

fn fixture() -> (Arc<ScriptedProc>, Herdr) {
    let p = Arc::new(ScriptedProc::new());
    let h = Herdr::new(
        p.clone(),
        "sess".to_owned(),
        vec![("TERM".to_owned(), "xterm".to_owned())],
    );
    (p, h)
}

fn name() -> TaskName {
    TaskName::parse("a").expect("valid name")
}

/// Args of the `i`th recorded call.
fn args(calls: &[Call], i: usize) -> Vec<String> {
    calls.get(i).map(|c| c.args.clone()).unwrap_or_default()
}

#[test]
fn create_worktree_argv_and_parse() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["worktree", "create"], ok(WT_CREATED));
    let wt = h
        .create_worktree(Path::new("/r"), &name(), Some("main"))
        .expect("created");
    assert_eq!(wt.path, PathBuf::from("/r/wt"));
    assert_eq!(wt.branch, "moor/a");
    let calls = p.calls();
    assert_eq!(
        args(&calls, 0),
        [
            "worktree",
            "create",
            "--cwd",
            "/r",
            "--branch",
            "moor/a",
            "--label",
            "moor:a",
            "--no-focus",
            "--base",
            "main"
        ]
    );
}

#[test]
fn create_worktree_reuses_the_worktree_already_on_the_branch() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "create"],
        Reply::Fail {
            code: 255,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    let wt = h
        .create_worktree(Path::new("/r"), &name(), None)
        .expect("reused");
    assert_eq!(wt.path, PathBuf::from("/r/wt"));
    assert_eq!(wt.branch, "moor/a");
}

#[test]
fn create_worktree_reuses_on_already_exists_code() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "create"],
        Reply::Fail {
            code: 1,
            stdout: r#"{"error":{"code":"worktree_already_exists","message":"x"}}"#.to_owned(),
            stderr: String::new(),
        },
    );
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    let wt = h
        .create_worktree(Path::new("/r"), &name(), None)
        .expect("reused");
    assert_eq!(wt.path, PathBuf::from("/r/wt"));
}

const CREATE_FAILED: &str = r#"{"error":{"code":"worktree_create_failed","message":"x"}}"#;

#[test]
fn create_worktree_reuses_stale_worktree_after_create_failed() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "create"],
        Reply::Fail {
            code: 1,
            stdout: CREATE_FAILED.to_owned(),
            stderr: String::new(),
        },
    );
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    let wt = h
        .create_worktree(Path::new("/r"), &name(), None)
        .expect("reused");
    assert_eq!(wt.path, PathBuf::from("/r/wt"));
    assert_eq!(wt.branch, "moor/a");
}

#[test]
fn create_worktree_create_failed_without_listed_worktree_errors() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "create"],
        Reply::Fail {
            code: 1,
            stdout: CREATE_FAILED.to_owned(),
            stderr: String::new(),
        },
    );
    p.on(
        Tool::Herdr,
        &["worktree", "list"],
        ok(r#"{"id":"cli:worktree:list","result":{"worktrees":[{"path":"/r/other","branch":"x"}]}}"#),
    );
    let err = h
        .create_worktree(Path::new("/r"), &name(), None)
        .expect_err("no worktree to reuse");
    assert!(
        matches!(&err, SurfaceError::Failed { summary } if summary == "herdr worktree_create_failed")
    );
}

#[test]
fn create_worktree_other_failure_does_not_reuse() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "create"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "boom".to_owned(),
        },
    );
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    let err = h
        .create_worktree(Path::new("/r"), &name(), None)
        .expect_err("no reuse");
    assert!(matches!(err, SurfaceError::Failed { .. }));
    assert!(p
        .calls()
        .iter()
        .all(|c| !c.args.iter().any(|a| a == "list")));
}

#[test]
fn every_call_sets_session_env_and_never_json() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    p.on(Tool::Herdr, &["workspace", "rename"], ok(""));
    p.on(Tool::Herdr, &["workspace", "close"], ok(""));
    let wt = Worktree {
        path: PathBuf::from("/r/wt"),
        branch: "moor/a".to_owned(),
    };
    let ws = h.ensure_workspace(&wt, "moor:a").expect("ensured");
    h.close_workspace(&ws).expect("closed");
    let calls = p.calls();
    assert_eq!(calls.len(), 3);
    for c in calls {
        assert!(c
            .env
            .contains(&("HERDR_SESSION".to_owned(), "sess".to_owned())));
        assert!(c.env.contains(&("TERM".to_owned(), "xterm".to_owned())));
        assert!(!c.args.iter().any(|a| a == "--json"));
    }
}

#[test]
fn ensure_workspace_reuses_open_workspace_and_renames() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    p.on(Tool::Herdr, &["workspace", "rename"], ok(""));
    let wt = Worktree {
        path: PathBuf::from("/r/wt"),
        branch: "moor/a".to_owned(),
    };
    let ws = h.ensure_workspace(&wt, "moor:a").expect("ensured");
    assert_eq!(ws.as_str(), "w9");
    let calls = p.calls();
    assert_eq!(args(&calls, 1), ["workspace", "rename", "w9", "moor:a"]);
    assert!(calls
        .iter()
        .all(|c| c.args.first().map(String::as_str) != Some("workspace")
            || c.args.get(1).map(String::as_str) != Some("create")));
}

#[test]
fn ensure_workspace_creates_when_none_open() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["worktree", "list"],
        ok(r#"{"result":{"worktrees":[]}}"#),
    );
    p.on(Tool::Herdr, &["workspace", "create"], ok(WS_CREATE));
    p.on(Tool::Herdr, &["workspace", "rename"], ok(""));
    let wt = Worktree {
        path: PathBuf::from("/r/wt"),
        branch: "b".to_owned(),
    };
    let ws = h.ensure_workspace(&wt, "moor:a").expect("ensured");
    assert_eq!(ws.as_str(), "w1");
    assert_eq!(
        args(&p.calls(), 1),
        ["workspace", "create", "--cwd", "/r/wt", "--no-focus"]
    );
}

#[test]
fn open_pane_closes_the_empty_tab_when_the_run_fails() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["tab", "create"], ok(TAB_CREATED));
    p.on(
        Tool::Herdr,
        &["pane", "run"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: "boom".to_owned(),
        },
    );
    p.on(Tool::Herdr, &["pane", "close"], ok(""));
    let cmd = PaneCmd {
        label: "agent:fix".to_owned(),
        argv: vec!["claude".to_owned()],
        cwd: None,
        env: Vec::new(),
    };
    assert!(h.open_pane(&WorkspaceRef::new("w1"), &cmd).is_err());
    let calls = p.calls();
    assert_eq!(args(&calls, 2), ["pane", "close", "w1:p2"]);
}

#[test]
fn open_pane_creates_tab_then_runs_quoted_command() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["tab", "create"], ok(TAB_CREATED));
    p.on(Tool::Herdr, &["pane", "run"], ok(""));
    let cmd = PaneCmd {
        label: "agent:fix".to_owned(),
        argv: vec!["claude".to_owned(), "hi there".to_owned()],
        cwd: Some(PathBuf::from("/r/wt")),
        env: vec![("K".to_owned(), "v".to_owned())],
    };
    let pane = h.open_pane(&WorkspaceRef::new("w1"), &cmd).expect("opened");
    assert_eq!(pane.as_str(), "w1:p2");
    let calls = p.calls();
    assert_eq!(
        args(&calls, 0),
        [
            "tab",
            "create",
            "--workspace",
            "w1",
            "--label",
            "agent:fix",
            "--cwd",
            "/r/wt",
            "--env",
            "K=v",
            "--no-focus"
        ]
    );
    assert_eq!(
        args(&calls, 1),
        ["pane", "run", "w1:p2", "claude 'hi there'"]
    );
}

#[test]
fn send_text_passes_text_as_one_argument() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["pane", "send-text"], ok(""));
    h.send_text(&PaneRef::new("p1"), "two words").expect("sent");
    assert_eq!(
        args(&p.calls(), 0),
        ["pane", "send-text", "p1", "two words"]
    );
}

#[test]
fn send_keys_names_each_key_in_one_call() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["pane", "send-keys"], ok(""));
    h.send_keys(
        &PaneRef::new("p1"),
        &[Key::CtrlU, Key::Digit(2), Key::CtrlM],
    )
    .expect("sent");
    assert_eq!(
        args(&p.calls(), 0),
        ["pane", "send-keys", "p1", "ctrl+u", "2", "ctrl+m"]
    );
}

#[test]
fn send_keys_rejects_a_two_digit_key_without_calling_herdr() {
    let (p, h) = fixture();
    assert!(matches!(
        h.send_keys(&PaneRef::new("p1"), &[Key::Digit(12)]),
        Err(SurfaceError::Failed { .. })
    ));
    assert!(p.calls().is_empty());
}

#[test]
fn send_to_missing_pane_maps_to_not_found() {
    let (p, h) = fixture();
    let body = r#"{"error":{"code":"pane_not_found","message":"x"}}"#;
    p.on(
        Tool::Herdr,
        &["pane", "send-text"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: body.to_owned(),
        },
    );
    assert_eq!(
        h.send_text(&PaneRef::new("p1"), "x"),
        Err(SurfaceError::NotFound)
    );
}

#[test]
fn remove_worktree_uses_open_workspace() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["worktree", "list"], ok(WT_LIST));
    p.on(Tool::Herdr, &["worktree", "remove"], ok(""));
    h.remove_worktree(Path::new("/r/wt")).expect("removed");
    assert_eq!(
        args(&p.calls(), 1),
        ["worktree", "remove", "--workspace", "w9", "--force"]
    );
}

#[test]
fn focus_workspace_runs_workspace_focus() {
    let (p, h) = fixture();
    p.on(Tool::Herdr, &["workspace", "focus"], ok(""));
    h.focus_workspace(&WorkspaceRef::new("w1"))
        .expect("focused");
    assert_eq!(args(&p.calls(), 0), ["workspace", "focus", "w1"]);
}

#[test]
fn focus_not_found_maps_to_not_found() {
    let (p, h) = fixture();
    let body = r#"{"error":{"code":"workspace_not_found","message":"x"}}"#;
    p.on(
        Tool::Herdr,
        &["workspace", "focus"],
        Reply::Fail {
            code: 1,
            stdout: String::new(),
            stderr: body.to_owned(),
        },
    );
    assert_eq!(
        h.focus_workspace(&WorkspaceRef::new("w1")),
        Err(SurfaceError::NotFound)
    );
}

#[test]
fn close_not_found_maps_to_not_found() {
    let (p, h) = fixture();
    let body = r#"{"error":{"code":"pane_not_found","message":"x"}}"#;
    p.on(
        Tool::Herdr,
        &["pane", "close"],
        Reply::Fail {
            code: 1,
            stdout: body.to_owned(),
            stderr: String::new(),
        },
    );
    assert_eq!(
        h.close_pane(&PaneRef::new("p")),
        Err(SurfaceError::NotFound)
    );
}

#[test]
fn server_not_running_starts_server_and_retries() {
    let (p, h) = fixture();
    p.on(
        Tool::Herdr,
        &["workspace", "close"],
        Reply::Fail {
            code: 1,
            stdout: NOT_RUNNING.to_owned(),
            stderr: String::new(),
        },
    );
    let err = h
        .close_workspace(&WorkspaceRef::new("w1"))
        .expect_err("down");
    assert!(matches!(err, SurfaceError::Unavailable(_)));
    let calls = p.calls();
    assert_eq!(args(&calls, 0), ["workspace", "close", "w1"]);
    assert_eq!(args(&calls, 1), ["server"]);
    assert!(calls
        .get(1)
        .map(|c| c.env.clone())
        .unwrap_or_default()
        .contains(&("HERDR_SESSION".to_owned(), "sess".to_owned())));
    assert_eq!(args(&calls, 2), ["workspace", "close", "w1"]);
}

#[test]
fn parses_research_samples_tolerantly() {
    assert_eq!(parse::workspace_created(WS_CREATE).expect("ws"), "w1");
    assert!(parse::worktree_list(PANE_LIST)
        .expect("no worktrees key")
        .is_empty());
    let c = parse::worktree_created(WT_CREATED).expect("wt");
    assert_eq!(c.workspace_id.as_deref(), Some("w2"));
    assert!(parse::tab_created("not json").is_err());
}
