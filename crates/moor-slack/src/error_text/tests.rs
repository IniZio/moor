//! The Slack wording is chosen by the error's kind, never by the controller's text.

use moor_core::response::{CloseRefusal, ErrorDetail, ErrorKind, ErrorView, Response};

use super::error_text;

fn failed(detail: Option<ErrorDetail>) -> Response {
    Response::err(
        "steer",
        ErrorView {
            kind: ErrorKind::Refused,
            message: "unrelated controller words".to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
            detail,
        },
    )
}

#[test]
fn each_kind_gets_slack_wording_without_cli_advice() {
    for detail in [
        ErrorDetail::AgentGone,
        ErrorDetail::FirstRunScreen,
        ErrorDetail::RelaunchCapped,
        ErrorDetail::LoginRequired,
        ErrorDetail::ProvisioningDead,
    ] {
        let text = error_text(failed(Some(detail.clone())));
        assert_ne!(text, "unrelated controller words", "{detail:?}");
        assert!(
            !text.replace("/moor ", "").contains("moor "),
            "{detail:?}: {text}"
        );
        assert!(!text.contains("--"), "{detail:?}: {text}");
    }
}

#[test]
fn an_error_without_a_kind_keeps_the_controller_message() {
    assert_eq!(error_text(failed(None)), "unrelated controller words");
    let close = ErrorDetail::CloseRefused(CloseRefusal::AlreadyClosed);
    assert_eq!(
        error_text(failed(Some(close))),
        "unrelated controller words"
    );
}

#[test]
fn a_reply_without_an_error_names_the_command() {
    let mut reply = failed(None);
    reply.error = None;
    assert_eq!(error_text(reply), "steer failed");
}
