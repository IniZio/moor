use super::*;

#[test]
fn errors_display() {
    assert_eq!(
        EngineError::Box(DriverError::NotFound).to_string(),
        "box: box not found"
    );
}

#[test]
fn launch_errors_display() {
    let failed = EngineError::Failed {
        step: "brief",
        message: "box file write failed".to_owned(),
    };
    assert_eq!(
        failed.to_string(),
        "launch failed at brief: box file write failed"
    );
    assert_eq!(EngineError::PaneGone.to_string(), "the agent pane is gone");
    assert_eq!(EngineError::NoSession.to_string(), "no agent session");
}
