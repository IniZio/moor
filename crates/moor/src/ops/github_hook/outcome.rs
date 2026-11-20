//! What came of one delivery, and the one log line built from it.
//!
//! The line names the delivery by its `X-GitHub-Delivery` id. The headers are not signed, so
//! each value is cut to plain printable text first. The secret, the signature and the body are
//! never written, not even in a parse error, which can quote the body.

use super::Pulled;

/// What came of a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Turned away, so nothing was recorded.
    Refused(Refusal),
    /// Signed, but an event or action moor does not handle.
    Ignored {
        /// The payload's `action`, when it has one.
        action: Option<String>,
    },
    /// A pull request event to handle. Its line waits for what it led to.
    Accepted(Pulled),
}

/// Why a delivery was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The controller has no App webhook secret.
    NoSecret,
    /// The signature was missing or wrong.
    BadSignature,
    /// A signed `pull_request` body that does not parse; carries the answer for GitHub, which
    /// may quote the body and so never reaches the log.
    Unparsable(String),
}

impl Outcome {
    /// The log line, without its delivery prefix, for a delivery of `event`; none for an
    /// accepted one, which is written once its event is handled.
    pub(super) fn line(&self, event: &str) -> Option<String> {
        let event = plain(event);
        Some(match self {
            Self::Refused(Refusal::NoSecret) => {
                format!("refused, no webhook secret is configured (event {event})")
            }
            Self::Refused(Refusal::BadSignature) => {
                format!("refused, bad or missing signature (event {event})")
            }
            Self::Refused(Refusal::Unparsable(_)) => {
                format!("refused, the {event} body does not parse")
            }
            Self::Ignored { action } => action.as_ref().map_or_else(
                || format!("ignored {event}"),
                |action| format!("ignored {event}/{}", plain(action)),
            ),
            Self::Accepted(_) => return None,
        })
    }
}

/// The line of an accepted `pulled`, with `led_to` the lines of what its workflows did; none
/// of them means no workflow wants it.
pub(super) fn accepted_line(pulled: &Pulled, led_to: &[String]) -> String {
    let event = &pulled.event;
    let led_to = if led_to.is_empty() {
        "no workflow wants it".to_owned()
    } else {
        led_to.join("; ")
    };
    format!(
        "accepted pull_request/{} {} at {}: {led_to}",
        plain(&pulled.action),
        event.target,
        event.head_sha.short()
    )
}

/// `line` of delivery `delivery`.
pub(super) fn delivery_line(delivery: Option<&str>, line: &str) -> String {
    format!("github delivery {}: {line}", plain(delivery.unwrap_or("")))
}

/// `text` cut to 64 printable ASCII characters, so a header cannot break the line; `-` when
/// nothing is left.
fn plain(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(char::is_ascii_graphic)
        .take(64)
        .collect();
    if kept.is_empty() {
        "-".to_owned()
    } else {
        kept
    }
}
