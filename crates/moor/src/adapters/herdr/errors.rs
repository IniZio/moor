//! Single mapping from herdr process failures to Surface errors.

use moor_core::seam::proc::ProcError;
use moor_core::seam::surface::SurfaceError;

/// What a failed herdr call means to the adapter.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HerdrFailure {
    /// No herdr server runs for this session; the caller may start one.
    ServerNotRunning,
    /// The target already exists (herdr exit 255 or an `*already_exists` code).
    AlreadyExists(SurfaceError),
    /// Any other failure, already in Surface terms.
    Surface(SurfaceError),
}

/// Error code and message parsed from herdr's JSON error body.
struct ErrorBody {
    /// Stable herdr error code.
    code: String,
    /// Human message from herdr.
    message: String,
}

/// Parse `{"error":{"code","message"}}` from one stream, tolerating extra fields and noise lines.
fn parse_body(text: &str) -> Option<ErrorBody> {
    text.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let error = value.get("error")?;
        Some(ErrorBody {
            code: error.get("code")?.as_str()?.to_owned(),
            message: error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    })
}

/// Whether a herdr error code names a missing pane, workspace or worktree.
fn is_not_found(code: &str) -> bool {
    code == "not_found" || code.ends_with("_not_found")
}

/// Map a body to a failure; codes are echoed, raw messages are not.
fn map_body(body: &ErrorBody) -> HerdrFailure {
    if body.code == "server_not_running" {
        return HerdrFailure::ServerNotRunning;
    }
    if body.code == "invalid_request" && body.message.contains("unknown variant") {
        return HerdrFailure::Surface(SurfaceError::Unavailable(
            "herdr does not support this request; restart the moor herdr session".to_owned(),
        ));
    }
    if body.code == "already_exists" || body.code.ends_with("_already_exists") {
        return HerdrFailure::AlreadyExists(SurfaceError::Failed {
            summary: format!("herdr {}", body.code),
        });
    }
    if is_not_found(&body.code) {
        return HerdrFailure::Surface(SurfaceError::NotFound);
    }
    HerdrFailure::Surface(SurfaceError::Failed {
        summary: format!("herdr {}", body.code),
    })
}

/// Map a failed herdr process to a [`HerdrFailure`].
pub(super) fn map_error(e: &ProcError) -> HerdrFailure {
    match e {
        ProcError::Timeout { .. } => HerdrFailure::Surface(SurfaceError::Timeout),
        ProcError::Spawn(_) => HerdrFailure::Surface(SurfaceError::Unavailable(
            "herdr could not be started".to_owned(),
        )),
        ProcError::Signalled(sig) => HerdrFailure::Surface(SurfaceError::Failed {
            summary: format!("herdr killed by signal {sig}"),
        }),
        ProcError::Interrupted => HerdrFailure::Surface(SurfaceError::Failed {
            summary: "herdr interrupted".to_owned(),
        }),
        ProcError::NonZero {
            code,
            stdout,
            stderr_tail,
        } => parse_body(stderr_tail)
            .or_else(|| parse_body(stdout))
            .map_or_else(
                || {
                    let err = SurfaceError::Failed {
                        summary: format!("herdr exited with code {code}"),
                    };
                    if *code == 255 {
                        HerdrFailure::AlreadyExists(err)
                    } else {
                        HerdrFailure::Surface(err)
                    }
                },
                |body| map_body(&body),
            ),
    }
}

#[cfg(test)]
mod tests;
