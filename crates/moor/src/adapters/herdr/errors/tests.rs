use std::time::Duration;

use moor_core::seam::proc::Tool;

use super::*;

fn nonzero(stdout: &str, stderr: &str) -> ProcError {
    ProcError::NonZero {
        code: 1,
        stdout: stdout.to_owned(),
        stderr_tail: stderr.to_owned(),
    }
}

fn body(code: &str, message: &str) -> String {
    format!(r#"{{"id":"cli:x","error":{{"code":"{code}","message":"{message}"}}}}"#)
}

#[test]
fn server_not_running_from_stderr() {
    let e = nonzero(
        "",
        &body("server_not_running", "no herdr server is running"),
    );
    assert_eq!(map_error(&e), HerdrFailure::ServerNotRunning);
}

#[test]
fn unknown_variant_means_restart_session() {
    let e = nonzero(
        &body(
            "invalid_request",
            "invalid request: unknown variant `x`, expected `ping`",
        ),
        "",
    );
    assert_eq!(
        map_error(&e),
        HerdrFailure::Surface(SurfaceError::Unavailable(
            "herdr does not support this request; restart the moor herdr session".to_owned()
        ))
    );
}

#[test]
fn not_found_codes_map_to_not_found() {
    for code in ["pane_not_found", "workspace_not_found", "not_found"] {
        let e = nonzero("", &body(code, "pane wGM:p2 not found"));
        assert_eq!(
            map_error(&e),
            HerdrFailure::Surface(SurfaceError::NotFound),
            "{code}"
        );
    }
}

#[test]
fn timeout_and_spawn() {
    let t = ProcError::Timeout {
        tool: Tool::Herdr,
        after: Duration::from_secs(1),
    };
    assert_eq!(map_error(&t), HerdrFailure::Surface(SurfaceError::Timeout));
    let s = ProcError::Spawn(std::io::Error::other("nope"));
    assert_eq!(
        map_error(&s),
        HerdrFailure::Surface(SurfaceError::Unavailable(
            "herdr could not be started".to_owned()
        ))
    );
}

#[test]
fn other_code_is_failed_without_raw_message() {
    let e = nonzero("", &body("invalid_resume_argv", "secret /home/x path"));
    assert_eq!(
        map_error(&e),
        HerdrFailure::Surface(SurfaceError::Failed {
            summary: "herdr invalid_resume_argv".to_owned()
        })
    );
}

#[test]
fn unparsable_output_is_failed_with_exit_code() {
    let e = nonzero("", "boom: raw text");
    assert_eq!(
        map_error(&e),
        HerdrFailure::Surface(SurfaceError::Failed {
            summary: "herdr exited with code 1".to_owned()
        })
    );
}
