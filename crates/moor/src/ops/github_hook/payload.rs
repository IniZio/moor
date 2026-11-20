//! The parts of a `pull_request` delivery moor reads, parsed into a [`PullEvent`].

use moor_core::name::ForgeRepo;
use moor_core::workflow::{CommitSha, PullTarget};
use serde::Deserialize;

use crate::ops::workflow::PullEvent;

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

/// The pull request event of delivery `event` with `body`; none for any other event or action,
/// which moor answers and drops.
///
/// # Errors
/// A `pull_request` body that does not parse.
pub(super) fn parse(event: &str, body: &[u8]) -> Result<Option<PullEvent>, String> {
    if event != "pull_request" {
        return Ok(None);
    }
    let bad = |e: &dyn std::fmt::Display| format!("not a pull_request payload: {e}");
    let p: Payload = serde_json::from_slice(body).map_err(|e| bad(&e))?;
    let repo = ForgeRepo::parse(&p.repository.full_name).map_err(|e| bad(&e))?;
    let target = PullTarget {
        repo,
        number: p.number,
    };
    Ok(match p.action.as_str() {
        "opened" | "reopened" | "ready_for_review" | "synchronize" => Some(PullEvent::Fire {
            target,
            head_sha: CommitSha::parse(&p.pull_request.head.sha).map_err(|e| bad(&e))?,
        }),
        "closed" => Some(PullEvent::Closed(target)),
        _ => None,
    })
}
