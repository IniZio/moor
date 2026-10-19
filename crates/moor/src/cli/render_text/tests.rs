//! Tests for text rendering.

use moor_core::config::Layer;
use moor_core::view::env::{
    EnvAgent, EnvCredential, EnvGithub, EnvKind, EnvMint, EnvProvider, EnvResolved, EnvRow,
    EnvView, ValueKind,
};
use moor_core::view::LaunchFailure;
use moor_core::{ErrorKind, ErrorView, Response, Warning};
use serde_json::json;

use super::{duration_text, render};

/// Builds a warning with `msg`.
fn warn(msg: &str) -> Warning {
    Warning {
        code: "w".into(),
        message: msg.into(),
    }
}

#[test]
fn version_line_is_exact() {
    let hash = "a".repeat(40);
    let v = json!({"version": "0.2.0", "commit": hash, "schema": 1});
    let (out, err) = render(&Response::ok("version", Some(v)), false);
    assert_eq!(out, format!("moor 0.2.0 {hash} schema=1\n"));
    assert_eq!(err, "");
}

#[test]
fn odd_version_value_falls_back_to_json() {
    let (out, _) = render(&Response::ok("version", Some(json!({"version": 3}))), false);
    assert!(out.contains("\"version\": 3"));
    assert!(out.ends_with('\n'));
}

#[test]
fn cp_prints_its_confirmation() {
    let v = json!({"copied": true, "confirmation": "copied a to b (3 bytes)"});
    let (out, _) = render(&Response::ok("cp", Some(v)), false);
    assert_eq!(out, "copied a to b (3 bytes)\n");
}

/// Builds a sleep/wake value for task `task` with an optional Box state.
fn sleep_value(task: &str, state: Option<&str>, woke: bool) -> serde_json::Value {
    let b = state.map(|s| json!({"state": s, "sleep": {"after": null, "reason": null}}));
    json!({"task": task, "driver": b, "woke": woke})
}

#[test]
fn sleep_prints_one_line() {
    let v = sleep_value("t1", Some("asleep"), false);
    let (out, _) = render(&Response::ok("sleep", Some(v)), false);
    assert_eq!(out, "t1 is asleep\n");
    let v = sleep_value("t1", Some("ready"), false);
    let (out, _) = render(&Response::ok("sleep", Some(v)), false);
    assert_eq!(out, "t1: box is ready\n");
    let v = sleep_value("t1", None, false);
    let (out, _) = render(&Response::ok("sleep", Some(v)), false);
    assert_eq!(out, "t1 is asleep\n");
}

#[test]
fn wake_prints_whether_it_woke() {
    let v = sleep_value("t1", Some("ready"), true);
    let (out, _) = render(&Response::ok("wake", Some(v)), false);
    assert_eq!(out, "t1 woke\n");
    let v = sleep_value("t1", Some("ready"), false);
    let (out, _) = render(&Response::ok("wake", Some(v)), false);
    assert_eq!(out, "t1 was not asleep\n");
}

#[test]
fn malformed_sleep_value_falls_back_to_json() {
    let (out, _) = render(&Response::ok("sleep", Some(json!({"woke": false}))), false);
    assert!(out.contains("\"woke\": false"));
    let (out, _) = render(&Response::ok("wake", Some(json!({"task": 3}))), false);
    assert!(out.contains("\"task\": 3"));
}

#[test]
fn other_value_is_pretty_json_and_none_is_empty() {
    let (out, _) = render(&Response::ok("show", Some(json!({"a": 1}))), false);
    assert_eq!(out, "{\n  \"a\": 1\n}\n");
    assert_eq!(
        render(&Response::ok("show", None), false),
        (String::new(), String::new())
    );
}

#[test]
fn error_prints_message_and_hints() {
    let e = ErrorView {
        kind: ErrorKind::Usage,
        message: "bad thing".into(),
        hints: vec!["try x".into(), "or y".into()],
        context: vec![],
    };
    let (out, err) = render(&Response::err("show", e), false);
    assert_eq!(out, "");
    assert_eq!(err, "moor: bad thing\nhint: try x\nhint: or y\n");
}

#[test]
fn warnings_print_unless_quiet() {
    let resp = Response::ok("show", None).with_warnings(vec![warn("careful")]);
    assert_eq!(render(&resp, false).1, "moor: warning: careful\n");
    assert_eq!(render(&resp, true).1, "");
}

/// Builds a row.
fn row(kind: EnvKind, name: &str, source: Layer, note: &str) -> EnvRow {
    EnvRow {
        kind,
        name: name.into(),
        source,
        value_kind: ValueKind::Plain,
        delivery: None,
        note: note.into(),
    }
}

/// Builds a credential check.
fn check(name: &str, ok: bool, reason: Option<&str>) -> EnvCredential {
    EnvCredential {
        kind: EnvKind::Credential,
        which: name.into(),
        ok,
        reason: reason.map(Into::into),
    }
}

/// Builds an env response from `rows` and `credentials`.
fn env_view(rows: Vec<EnvRow>, credentials: Vec<EnvCredential>) -> Response {
    let github = EnvGithub {
        identity: "auto".into(),
        app_id: None,
        login: "logged_out".into(),
        mint: EnvMint {
            status: "skipped".into(),
            error: None,
            reason: Some("GitHub App not set up".into()),
        },
        resolved: EnvResolved {
            mode: None,
            app_installed: "n/a".into(),
            token_source: None,
            error: Some("no app".into()),
        },
    };
    let view = EnvView {
        repo: "/r".into(),
        agent: EnvAgent { model: None },
        github,
        rows,
        provider: EnvProvider {
            name: "local".into(),
            source: Layer::Default,
            size: "small".into(),
            size_source: Layer::Default,
        },
        credentials,
    };
    Response::ok("env", Some(serde_json::to_value(view).unwrap()))
}

#[test]
fn env_text_starts_with_agent_and_marks_credentials() {
    let rows = vec![
        row(EnvKind::Credential, "GOOD", Layer::User, "env: X"),
        row(EnvKind::Credential, "BAD", Layer::Repo, "file: /x"),
        row(EnvKind::Box, "size", Layer::Default, "small"),
    ];
    let credentials = vec![
        check("GOOD", true, None),
        check("BAD", false, Some("missing")),
    ];
    let (out, _) = render(&env_view(rows, credentials), false);
    assert!(out.starts_with("agent"));
    let lines: Vec<&str> = out.lines().collect();
    let good = lines.iter().find(|l| l.contains("GOOD")).unwrap();
    assert!(good.contains('\u{2713}') && !good.contains('\u{2717}'));
    let bad = lines.iter().find(|l| l.contains("BAD")).unwrap();
    assert!(bad.contains('\u{2717}') && !bad.contains('\u{2713}'));
    assert!(bad.ends_with("(missing)"));
    assert!(out.contains("github token: skipped: GitHub App not set up"));
    assert!(out.contains("github identity: unknown: no app"));
    assert!(lines
        .last()
        .unwrap()
        .starts_with("- box size  [default]  small"));
}

/// A successful `run` value.
fn run_value() -> serde_json::Value {
    json!({"ran": true, "exit": 0, "interrupted": false, "sha": "abc"})
}

#[test]
fn ok_run_prints_nothing_on_stdout() {
    let (out, err) = render(&Response::ok("run", Some(run_value())), false);
    assert_eq!(out, "");
    assert_eq!(err, "");
}

#[test]
fn failed_run_renders_error_on_stderr() {
    let e = ErrorView {
        kind: ErrorKind::Failed,
        message: "no box".into(),
        hints: vec![],
        context: vec![],
    };
    let (out, err) = render(&Response::err("run", e), false);
    assert_eq!(out, "");
    assert_eq!(err, "moor: no box\n");
}

#[test]
fn quiet_run_drops_warnings_and_stays_empty() {
    let mut resp = Response::ok("run", Some(run_value()));
    resp.warnings.push(warn("careful"));
    assert_eq!(render(&resp, true), (String::new(), String::new()));
    let (out, err) = render(&resp, false);
    assert_eq!(out, "");
    assert_eq!(err, "moor: warning: careful\n");
}

#[test]
fn steer_prints_one_line() {
    let v = json!({"task": "t1", "sent": "hi", "answered_menu": false});
    let (out, _) = render(&Response::ok("steer", Some(v)), false);
    assert_eq!(out, "steered t1\n");
}

#[test]
fn end_session_prints_ordinal_id_and_box_kept() {
    let v = json!({"task": "t1", "session": 7, "ordinal": 2, "box_kept": true});
    let (out, _) = render(&Response::ok("end-session", Some(v)), false);
    assert_eq!(out, "ended Session 2 (id 7) in t1; Box kept open\n");
}

#[test]
fn open_session_prints_ordinal_id_and_pane() {
    let v = json!({"task": "t1", "session": 9, "ordinal": 3, "pane": "p4"});
    let (out, _) = render(&Response::ok("open-session", Some(v)), false);
    assert_eq!(out, "opened Session 3 (id 9) in t1; pane p4\n");
    let v = json!({"task": "t1", "session": 9, "ordinal": 3, "pane": null});
    let (out, _) = render(&Response::ok("open-session", Some(v)), false);
    assert_eq!(out, "opened Session 3 (id 9) in t1\n");
}

#[test]
fn open_host_session_prints_id_and_workdir() {
    let v = json!({"session": 12, "workdir": "/dir", "state": "open"});
    let (out, _) = render(&Response::ok("open-host-session", Some(v)), false);
    assert_eq!(out, "host Session 12 working in /dir\n");
}

#[test]
fn end_host_session_prints_id_and_workdir() {
    let v = json!({"session": 3, "workdir": "/dir", "state": "ended"});
    let (out, _) = render(&Response::ok("end-host-session", Some(v)), false);
    assert_eq!(out, "host Session 3 working in /dir\n");
}

/// A Box named `name` in `lifecycle`, with an optional Box state.
fn lifecycle_task(name: &str, lifecycle: &str, box_state: Option<&str>) -> serde_json::Value {
    let b = box_state.map(|s| json!({"state": s, "provider": "local"}));
    json!({"id": 7, "name": name, "lifecycle": lifecycle, "session_state": "idle", "box": b})
}

#[test]
fn start_prints_name_state_box_and_how_to_open() {
    let v = json!({"task": lifecycle_task("alpha", "ready", Some("ready")), "attached": false});
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(
        out,
        "started alpha (#7): ready/idle, box ready\nopen it with: moor open alpha\n"
    );
}

#[test]
fn start_says_setup_failed_and_keeps_the_open_hint() {
    let v = json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "setup": {"ran": true, "ok": false, "line": "make: *** [deps] Error 2"},
        "warnings_inline": []
    });
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(
        out,
        "started alpha (#7): ready/idle, box ready, setup failed\nopen it with: moor open alpha\n"
    );
}

#[test]
fn start_names_the_failed_step_its_tail_the_box_and_how_to_close_it() {
    let v = json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "setup": {"ran": true, "ok": false, "tail": "npm ERR! missing\nexit 1"},
        "failed_step": "setup",
        "box": "box-9",
    });
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(
        out,
        "started alpha (#7): ready/idle, box ready, setup failed\nopen it with: moor open alpha\n\
         setup failed in Box box-9:\n  npm ERR! missing\n  exit 1\nclose it with: moor close alpha\n"
    );
}

#[test]
fn start_does_not_say_setup_failed_when_setup_passed_or_did_not_run() {
    for setup in [
        json!({"ran": true, "ok": true}),
        json!({"ran": false, "ok": false}),
    ] {
        let v = json!({"task": lifecycle_task("alpha", "ready", Some("ready")), "setup": setup});
        let (out, _) = render(&Response::ok("start", Some(v)), false);
        assert_eq!(
            out,
            "started alpha (#7): ready/idle, box ready\nopen it with: moor open alpha\n"
        );
    }
}

#[test]
fn start_says_attached_and_queued() {
    let v = json!({"task": lifecycle_task("alpha", "ready", Some("ready")), "attached": true});
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert!(out.starts_with("attached alpha (#7): ready/idle, box ready\n"));
    let v = json!({"task": lifecycle_task("alpha", "waiting_for_box", None), "queued": true});
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(out, "queued alpha (#7): waiting for a free box\n");
}

#[test]
fn start_omits_open_hint_for_a_box_that_cannot_be_opened() {
    let v = json!({"task": lifecycle_task("alpha", "ready", Some("failed"))});
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(out, "started alpha (#7): ready/idle, box failed\n");
}

#[test]
fn start_prints_inline_warnings_and_promotions() {
    let v = json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "promoted": ["beta", "gamma"],
        "warnings_inline": ["agent dispatch failed: no pane"]
    });
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(
        out,
        "started alpha (#7): ready/idle, box ready\nopen it with: moor open alpha\n\
         started from the queue: beta, gamma\nwarning: agent dispatch failed: no pane\n"
    );
}

#[test]
fn start_keeps_its_output_for_a_launched_agent() {
    let v = json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "agent": {"launched": true, "session": "s", "reason": null}
    });
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert_eq!(
        out,
        "started alpha (#7): ready/idle, box ready\nopen it with: moor open alpha\n"
    );
}

#[test]
fn start_shows_a_launch_step_failure_as_a_warning() {
    let reason = LaunchFailure {
        step: "hooks".to_owned(),
        message: "hook install failed".to_owned(),
        tail: None,
        at: "2026-01-01T00:00:00Z".to_owned(),
    };
    let v = json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "agent": {"launched": false, "session": null, "reason": reason},
        "warnings_inline": [reason.text()]
    });
    let (out, _) = render(&Response::ok("start", Some(v)), false);
    assert!(!out.contains("launched under"));
    assert!(out.ends_with(
        "warning: agent dispatch failed: hooks: hook install failed; re-run start to retry\n"
    ));
}

#[test]
fn close_prints_name_and_box_outcome() {
    let v = json!({
        "task": lifecycle_task("alpha", "closed", None),
        "box_deleted": true, "worktree_kept": true, "promoted": []
    });
    let (out, _) = render(&Response::ok("close", Some(v)), false);
    assert_eq!(out, "closed alpha (#7), box deleted\n");
    let v = json!({"task": lifecycle_task("alpha", "closed", None), "box_deleted": false});
    let (out, _) = render(&Response::ok("close", Some(v)), false);
    assert_eq!(out, "closed alpha (#7), box kept\n");
}

#[test]
fn close_prints_transcript_and_promotions() {
    let v = json!({
        "task": lifecycle_task("alpha", "closed", None),
        "box_deleted": true, "transcript": "/t/alpha.md", "promoted": ["beta"]
    });
    let (out, _) = render(&Response::ok("close", Some(v)), false);
    assert_eq!(
        out,
        "closed alpha (#7), box deleted\ntranscript: /t/alpha.md\nstarted from the queue: beta\n"
    );
}

fn wait_value(cond: &str, timed_out: bool, waited_ms: u64) -> serde_json::Value {
    json!({
        "task": lifecycle_task("alpha", "ready", Some("ready")),
        "timed_out": timed_out,
        "waited_ms": waited_ms,
        "for": cond,
        "run": null,
    })
}

#[test]
fn wait_prints_one_line_when_the_condition_held() {
    let v = wait_value("idle", false, 252_000);
    let (out, _) = render(&Response::ok("wait", Some(v)), false);
    assert_eq!(out, "alpha (#7): idle after 4m12s\n");
}

#[test]
fn wait_says_timed_out() {
    let v = wait_value("done", true, 30_000);
    let (out, _) = render(&Response::ok("wait", Some(v)), false);
    assert_eq!(out, "alpha (#7): timed out waiting for done after 30s\n");
}

#[test]
fn wait_durations_scale_from_milliseconds_to_hours() {
    assert_eq!(duration_text(250), "250ms");
    assert_eq!(duration_text(3_723_000), "1h02m03s");
}
