//! Why a Box closed, and who closed it: the `reason` and `by` on a `closed` event.

/// Why a Box closed. Every close names one, so a person reading the Box's history, or the run it
/// ended, can tell who or what closed it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum CloseReason {
    /// A person asked for the close: `moor close`, the MCP `close_box` tool or the portal.
    Requested {
        /// The identity the request acted as.
        by: String,
    },
    /// A Box stayed in `setup_failed` past the grace period and the controller closed it.
    SetupFailedExpired,
    /// A workflow run ended, and its Box saw no steer or terminal for the grace period after.
    RunEnded,
    /// The Box's pull request merged.
    PrMerged,
    /// The Box's pull request closed without a merge.
    PrClosed,
    /// A one-off Box saw no activity for the idle period.
    Idle,
    /// `close-failed` closed the Box.
    CloseFailed,
    /// The pull request run the Box was started for was cancelled before its agent launched.
    WorkflowCancelled,
    /// The run's agent did not start within the start limit of its firing.
    StartLimit,
    /// The run the Box was started for had already ended when its start was recorded.
    LateFiring,
    /// A newer run of the same workflow started, and this Box held nothing worth keeping.
    Superseded,
}

impl CloseReason {
    /// The close a person asked for, acting as `by`.
    #[must_use]
    pub fn requested(by: impl Into<String>) -> Self {
        Self::Requested { by: by.into() }
    }

    /// The `reason` the `closed` event carries.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Requested { .. } => "requested",
            Self::SetupFailedExpired => "setup_failed_expired",
            Self::RunEnded => "run_ended",
            Self::PrMerged => "pr_merged",
            Self::PrClosed => "pr_closed",
            Self::Idle => "idle",
            Self::CloseFailed => "close_failed",
            Self::WorkflowCancelled => "workflow_cancelled",
            Self::StartLimit => "start_limit",
            Self::LateFiring => "late_firing",
            Self::Superseded => "superseded",
        }
    }

    /// The person who asked for the close, for a close a person asked for.
    #[must_use]
    pub fn by(&self) -> Option<&str> {
        match self {
            Self::Requested { by } => Some(by),
            _ => None,
        }
    }

    /// The reason a `closed` event's `reason` and `by` name; none for a code this build does
    /// not know, or a person's close that does not say who.
    #[must_use]
    pub fn from_event(reason: &str, by: Option<&str>) -> Option<Self> {
        Some(match reason {
            "requested" => Self::requested(by?),
            "setup_failed_expired" => Self::SetupFailedExpired,
            "run_ended" => Self::RunEnded,
            "pr_merged" => Self::PrMerged,
            "pr_closed" => Self::PrClosed,
            "idle" => Self::Idle,
            "close_failed" => Self::CloseFailed,
            "workflow_cancelled" => Self::WorkflowCancelled,
            "start_limit" => Self::StartLimit,
            "late_firing" => Self::LateFiring,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }

    /// The one string a store column holds it as: the code, and for a person's close
    /// `requested:<by>`.
    #[must_use]
    pub fn stored(&self) -> String {
        match self {
            Self::Requested { by } => format!("requested:{by}"),
            other => other.code().to_owned(),
        }
    }

    /// The reason [`Self::stored`] wrote; none for one this build does not know.
    #[must_use]
    pub fn from_stored(s: &str) -> Option<Self> {
        match s.split_once(':') {
            Some(("requested", by)) if !by.is_empty() => Some(Self::requested(by)),
            Some(_) => None,
            None => Self::from_event(s, None),
        }
    }

    /// Who or what closed the Box, as the end of "its Box was closed ...".
    #[must_use]
    pub fn says(&self) -> String {
        match self {
            Self::Requested { by } => return format!("by {by}"),
            Self::SetupFailedExpired => "by moor, as its setup failed and nobody retried it",
            Self::RunEnded => "by moor, as its run had ended",
            Self::PrMerged => "by moor, as its pull request merged",
            Self::PrClosed => "by moor, as its pull request closed",
            Self::Idle => "by moor, as it sat idle",
            Self::CloseFailed => "by close-failed",
            Self::WorkflowCancelled => "by moor, as its run was cancelled",
            Self::StartLimit => "by moor, as its agent did not start in time",
            Self::LateFiring => "by moor, as its run had already ended",
            Self::Superseded => "by moor, as a newer run of its workflow started",
        }
        .to_owned()
    }
}

/// How a `closed` event carries its reason: the code as `reason`, and `by` for a person's close.
pub mod wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::CloseReason;

    /// The event's fields.
    #[derive(Serialize, Deserialize)]
    struct Fields {
        /// The code.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Who asked for the close.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    }

    /// Writes `reason` as the event's fields; none writes neither.
    ///
    /// # Errors
    /// The serializer's.
    #[allow(clippy::ref_option, reason = "serde's `with` hands over a reference")]
    pub fn serialize<S: Serializer>(reason: &Option<CloseReason>, s: S) -> Result<S::Ok, S::Error> {
        Fields {
            reason: reason.as_ref().map(|r| r.code().to_owned()),
            by: reason.as_ref().and_then(CloseReason::by).map(str::to_owned),
        }
        .serialize(s)
    }

    /// Reads the event's fields; a code this build does not know reads as none.
    ///
    /// # Errors
    /// The deserializer's.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CloseReason>, D::Error> {
        let f = Fields::deserialize(d)?;
        Ok(f.reason
            .and_then(|r| CloseReason::from_event(&r, f.by.as_deref())))
    }
}

impl From<CloseReason> for String {
    fn from(r: CloseReason) -> Self {
        r.stored()
    }
}

impl TryFrom<String> for CloseReason {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::from_stored(&s).ok_or_else(|| format!("unknown close reason `{s}`"))
    }
}

#[cfg(test)]
mod tests;
