//! Tests for pane observation through herdr.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::seam::proc::{Deadline, Tool};
use moor_core::seam::surface::{PaneRef, Surface, SurfaceError};

use crate::adapters::herdr::Herdr;
use crate::testing::proc::{Reply, ScriptedProc};

const DEADLINE: Deadline = Deadline(Duration::from_secs(5));

fn list(status: Option<&str>) -> String {
    let extra = status.map_or_else(String::new, |s| format!(r#""agent_status":"{s}","#));
    format!(
        r#"{{"id":"cli:pane:list","result":{{"panes":[{{"pane_id":"w1:p0","agent_status":"idle"}},{{{extra}"pane_id":"w1:p1","cwd":"/x"}}],"type":"pane_list"}}}}"#
    )
}

fn fixture(status: Option<&str>, tail: &str) -> (Arc<ScriptedProc>, Herdr) {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Herdr, &["pane", "list"], Reply::Ok(list(status)));
    p.on(Tool::Herdr, &["pane", "read"], Reply::Ok(tail.to_owned()));
    let h = Herdr::new(p.clone(), "sess".to_owned(), Vec::new());
    (p, h)
}

fn obs(h: &Herdr) -> Result<PaneObs, SurfaceError> {
    h.pane_obs(&PaneRef::new("w1:p1"), DEADLINE)
}

#[test]
fn status_words_map_to_pane_status() {
    for (word, want) in [
        ("working", PaneStatus::Working),
        ("idle", PaneStatus::Idle),
        ("blocked", PaneStatus::Blocked),
        ("unknown", PaneStatus::Unknown),
        ("something-new", PaneStatus::Unknown),
    ] {
        let (_, h) = fixture(Some(word), "");
        assert_eq!(obs(&h).expect("observed").status, want, "{word}");
    }
}

#[test]
fn missing_status_field_is_unknown() {
    let (_, h) = fixture(None, "");
    assert_eq!(obs(&h).expect("observed").status, PaneStatus::Unknown);
}

#[test]
fn pane_absent_from_list_is_not_found_and_is_not_read() {
    let (p, h) = fixture(Some("idle"), "text");
    assert_eq!(
        h.pane_obs(&PaneRef::new("w9:p9"), DEADLINE),
        Err(SurfaceError::NotFound)
    );
    assert_eq!(p.calls().len(), 1);
}

#[test]
fn tail_comes_from_pane_read() {
    let (p, h) = fixture(Some("working"), "line one\nline two\n");
    let got = obs(&h).expect("observed");
    assert_eq!(got.tail, "line one\nline two");
    assert_eq!(got.exit_marker, None);
    let calls = p.calls();
    let read = calls.get(1).expect("read call");
    assert_eq!(
        read.args.get(..3),
        Some(&["pane", "read", "w1:p1"].map(str::to_owned)[..])
    );
    assert!(read.args.contains(&"--lines".to_owned()));
}

#[test]
fn exit_marker_sets_the_code() {
    let (_, h) = fixture(Some("idle"), "bye\nmoor: agent exited 7\n");
    assert_eq!(obs(&h).expect("observed").exit_marker, Some(7));
}

#[test]
fn last_exit_marker_wins() {
    let (_, h) = fixture(
        Some("idle"),
        "moor: agent exited 1\nmore output\nmoor: agent exited 0\n",
    );
    assert_eq!(obs(&h).expect("observed").exit_marker, Some(0));
}

#[test]
fn exit_marker_mid_line_is_ignored() {
    let (_, h) = fixture(Some("idle"), "$ foo; echo moor: agent exited 3\n");
    assert_eq!(obs(&h).expect("observed").exit_marker, None);
}

#[test]
fn exit_marker_ignores_residue_after_the_code() {
    let (_, h) = fixture(Some("idle"), "moor: agent exited 255sc to cancel\n");
    assert_eq!(obs(&h).expect("observed").exit_marker, Some(255));
}

#[test]
fn exit_marker_ignores_unrelated_text() {
    let (_, h) = fixture(
        Some("idle"),
        "sc to cancel moor: agent exited\nagent exited 4\n",
    );
    assert_eq!(obs(&h).expect("observed").exit_marker, None);
}

#[test]
fn exit_marker_needs_a_number() {
    let (_, h) = fixture(Some("idle"), "moor: agent exited soon\n");
    assert_eq!(obs(&h).expect("observed").exit_marker, None);
}

#[test]
fn hung_list_times_out() {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Herdr, &["pane", "list"], Reply::Timeout);
    let h = Herdr::new(p, "sess".to_owned(), Vec::new());
    assert_eq!(obs(&h), Err(SurfaceError::Timeout));
}

#[test]
fn hung_read_times_out() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Herdr,
        &["pane", "list"],
        Reply::Ok(list(Some("idle"))),
    );
    p.on(Tool::Herdr, &["pane", "read"], Reply::Timeout);
    let h = Herdr::new(p, "sess".to_owned(), Vec::new());
    assert_eq!(obs(&h), Err(SurfaceError::Timeout));
}

#[test]
fn each_call_carries_the_deadline() {
    let (p, h) = fixture(Some("idle"), "");
    obs(&h).expect("observed");
    for c in p.calls() {
        assert_eq!(c.deadline, DEADLINE);
    }
}

#[test]
fn failures_carry_no_raw_stderr() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Herdr,
        &["pane", "list"],
        Reply::Fail {
            code: 2,
            stdout: String::new(),
            stderr: "secret-token /home/me".to_owned(),
        },
    );
    let h = Herdr::new(p, "sess".to_owned(), Vec::new());
    let msg = format!("{:?}", obs(&h).expect_err("failed"));
    assert!(!msg.contains("secret-token"), "{msg}");
}

#[test]
fn read_of_a_vanished_pane_is_not_found() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Herdr,
        &["pane", "list"],
        Reply::Ok(list(Some("idle"))),
    );
    p.on(
        Tool::Herdr,
        &["pane", "read"],
        Reply::Fail {
            code: 1,
            stdout: r#"{"error":{"code":"pane_not_found","message":"x"}}"#.to_owned(),
            stderr: String::new(),
        },
    );
    let h = Herdr::new(p, "sess".to_owned(), Vec::new());
    assert_eq!(obs(&h), Err(SurfaceError::NotFound));
}

#[test]
fn down_server_is_unavailable_and_is_not_started() {
    let p = Arc::new(ScriptedProc::new());
    p.on(
        Tool::Herdr,
        &["pane", "list"],
        Reply::Fail {
            code: 1,
            stdout: r#"{"error":{"code":"server_not_running","message":"x"}}"#.to_owned(),
            stderr: String::new(),
        },
    );
    let h = Herdr::new(p.clone(), "sess".to_owned(), Vec::new());
    assert!(matches!(obs(&h), Err(SurfaceError::Unavailable(_))));
    assert_eq!(p.calls().len(), 1);
}
