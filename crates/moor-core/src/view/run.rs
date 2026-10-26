//! Typed `value` payloads for `run`, `wait` and `cp`.

use crate::name::RunId;
use crate::request::WaitFor;
use crate::view::BoxView;

/// Payload of `run`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct RunValue {
    /// Whether the command ran.
    pub ran: bool,
    /// Run id; null when refused before a run row exists.
    pub run: Option<RunId>,
    /// Exit code; null while unfinished or when killed by a signal.
    pub exit: Option<i32>,
    /// Whether the run was interrupted.
    pub interrupted: bool,
    /// Sha a record run executed against; null for any other run.
    pub sha: Option<String>,
    /// Record text, when one was captured.
    pub record: Option<String>,
    /// Log file path.
    pub log: String,
}

/// Run details inside a `wait` payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WaitRunView {
    /// Run id.
    pub id: RunId,
    /// Command argv.
    pub argv: Vec<String>,
    /// Whether the run is still in flight.
    pub in_flight: bool,
    /// Exit code, when finished.
    pub exit: Option<i32>,
    /// Whether the run was interrupted.
    pub interrupted: bool,
}

/// Payload of `wait`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WaitValue {
    /// The Box as last seen.
    #[serde(rename = "box")]
    pub boxed: BoxView,
    /// Whether the timeout elapsed before the condition held.
    pub timed_out: bool,
    /// Milliseconds spent waiting.
    pub waited_ms: u64,
    /// Condition that was waited for.
    #[serde(rename = "for")]
    pub wait_for: WaitFor,
    /// The run involved, when there is one.
    pub run: Option<WaitRunView>,
}

/// Payload of `cp`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CpValue {
    /// Whether the copy happened.
    pub copied: bool,
    /// Source path.
    pub from: String,
    /// Destination path.
    pub to: String,
    /// Bytes copied.
    pub bytes: u64,
    /// Confirmation text.
    pub confirmation: String,
}

/// Payload of `box-mirror`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MirrorValue {
    /// Work tree mirrored, in the Box.
    pub from: String,
    /// Destination directory on the host.
    pub to: String,
    /// Regular files this mirror transferred; unchanged files are not counted.
    pub files: u64,
    /// Bytes that came over the link from the Box.
    pub bytes: u64,
    /// Confirmation text.
    pub confirmation: String,
}

#[cfg(test)]
mod tests;
