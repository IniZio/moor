//! Tests for the timing event payloads.

use super::*;

#[test]
fn a_setup_line_serializes_its_position_and_timing() {
    let start = Event::Timing(TimingEvent::SetupLineStarted {
        index: 3,
        total: 6,
        line: "make setup".to_owned(),
    });
    assert_eq!(start.kind(), "setup_line_started");
    assert_eq!(
        start.payload(),
        serde_json::json!({"index": 3, "total": 6, "line": "make setup"})
    );
    let done = Event::Timing(TimingEvent::SetupLineFinished {
        index: 3,
        total: 6,
        ms: 12_000,
        ok: false,
    });
    assert_eq!(done.kind(), "setup_line_finished");
    assert_eq!(
        done.payload(),
        serde_json::json!({"index": 3, "total": 6, "ms": 12_000, "ok": false})
    );
}

#[test]
fn a_post_clone_step_serializes_its_name_and_timing() {
    let done = Event::Timing(TimingEvent::StepFinished {
        step: PostCloneStep::AgentInstall,
        ms: 41_000,
        ok: true,
    });
    assert_eq!(done.kind(), "step_finished");
    assert_eq!(
        done.payload(),
        serde_json::json!({"step": "agent_install", "ms": 41_000, "ok": true})
    );
}
