//! The parts of a `pull_request` delivery moor reads, parsed into a [`PullEvent`].

use moor_core::name::ForgeRepo;
use moor_core::workflow::{CommitSha, PullTarget};
use serde::Deserialize;

use crate::ops::workflow::{PullAction, PullEvent};

/// A `pull_request` payload, cut to what moor reads.
#[derive(Deserialize)]
struct Payload {
    /// Such as `opened` or `synchronize`.
    action: String,
    /// The pull request's number.
    number: u64,
    /// The pull request.
    pull_request: Pull,
    /// The base repo.
    repository: Repository,
}

/// The pull request of a payload.
#[derive(Deserialize)]
struct Pull {
    /// Its head.
    head: Head,
}

/// A pull request's head.
#[derive(Deserialize)]
struct Head {
    /// The head commit.
    sha: String,
}

/// The base repo of a payload.
#[derive(Deserialize)]
struct Repository {
    /// `OWNER/NAME`.
    full_name: String,
}

/// What a signed delivery holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parsed {
    /// A pull request event moor handles.
    Pull(Pulled),
    /// Any other event or action, which moor answers and drops.
    Other {
        /// The payload's `action`, when it has one.
        action: Option<String>,
    },
}

/// A pull request event and the action its payload named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pulled {
    /// Such as `opened`.
    pub(crate) action: String,
    /// The event.
    pub(crate) event: PullEvent,
}

/// Just the action of a payload of any event.
#[derive(Deserialize)]
struct Action {
    /// Absent from `ping`.
    action: Option<String>,
}

/// The delivery of `event` with `body`.
///
/// # Errors
/// A `pull_request` body that does not parse.
pub(super) fn parse(event: &str, body: &[u8]) -> Result<Parsed, String> {
    if event != "pull_request" {
        let action = serde_json::from_slice::<Action>(body)
            .ok()
            .and_then(|a| a.action);
        return Ok(Parsed::Other { action });
    }
    let bad = |e: &dyn std::fmt::Display| format!("not a pull_request payload: {e}");
    let p: Payload = serde_json::from_slice(body).map_err(|e| bad(&e))?;
    let repo = ForgeRepo::parse(&p.repository.full_name).map_err(|e| bad(&e))?;
    let target = PullTarget {
        repo,
        number: p.number,
    };
    let action = match p.action.as_str() {
        "opened" | "reopened" | "ready_for_review" | "synchronize" => PullAction::Fire,
        "closed" => PullAction::Close,
        _ => {
            return Ok(Parsed::Other {
                action: Some(p.action),
            })
        }
    };
    Ok(Parsed::Pull(Pulled {
        action: p.action,
        event: PullEvent {
            target,
            head_sha: CommitSha::parse(&p.pull_request.head.sha).map_err(|e| bad(&e))?,
            action,
        },
    }))
}
