//! A refused start is worded from the refusal's kind, not from the controller's text.

use moor_core::response::{ErrorDetail, ErrorKind, ErrorView, Response};

use crate::worker::{start_outcome, Outcome};

#[test]
fn a_start_that_needs_a_login_tells_the_user_to_link_a_token() {
    let error = ErrorView {
        kind: ErrorKind::Refused,
        message: "words that name no command".to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
        detail: Some(ErrorDetail::LoginRequired),
    };
    let Outcome::Refused(text) = start_outcome(Response::err("start", error)) else {
        panic!("the start was not refused");
    };
    assert!(text.contains("`/moor login`"), "{text}");
    assert!(
        !text.replace("/moor ", "").contains("moor ") && !text.contains("--"),
        "{text}"
    );
}
