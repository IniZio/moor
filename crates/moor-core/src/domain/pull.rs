//! A pull request whose head is a Box's branch, and the one state a front shows for it.

use serde::{Deserialize, Serialize};

/// A pull request as the forge reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pull {
    /// The pull request number.
    pub number: u64,
    /// Where it stands.
    pub state: PullState,
    /// What the checks on its head commit say; none when it has none or is no longer open.
    pub checks: Option<Checks>,
}

/// A branch on the forge: whether it was pushed, and its pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPull {
    /// Whether the branch exists on the forge.
    pub pushed: bool,
    /// Its pull request; none when it has none yet.
    pub pull: Option<Pull>,
}

/// Where a pull request stands, in the order it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullState {
    /// Open as a draft.
    Draft,
    /// Open and ready for review.
    Open,
    /// Open with a review that asks for changes.
    ChangesRequested,
    /// Open with an approving review.
    Approved,
    /// Merged.
    Merged,
    /// Closed without a merge.
    Closed,
}

impl PullState {
    /// The state of a pull request GitHub reports as `state` (`OPEN`, `CLOSED` or `MERGED`),
    /// a draft or not, with review decision `review`; none for a state GitHub does not name.
    #[must_use]
    pub fn of(state: &str, draft: bool, review: Option<&str>) -> Option<Self> {
        Some(match (state, draft, review) {
            ("MERGED", ..) => Self::Merged,
            ("CLOSED", ..) => Self::Closed,
            ("OPEN", true, _) => Self::Draft,
            ("OPEN", false, Some("APPROVED")) => Self::Approved,
            ("OPEN", false, Some("CHANGES_REQUESTED")) => Self::ChangesRequested,
            ("OPEN", false, _) => Self::Open,
            _ => return None,
        })
    }

    /// Whether it is still open, so its checks still matter.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(
            self,
            Self::Draft | Self::Open | Self::ChangesRequested | Self::Approved
        )
    }
}

/// What the checks on a pull request's head commit say, all together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    /// Every check passed.
    Passing,
    /// A check failed.
    Failing,
    /// A check still runs or is expected.
    Pending,
}

impl Checks {
    /// The checks GitHub's status rollup `state` stands for; none for a state it does not name.
    #[must_use]
    pub fn of(rollup: &str) -> Option<Self> {
        match rollup {
            "SUCCESS" => Some(Self::Passing),
            "FAILURE" | "ERROR" => Some(Self::Failing),
            "PENDING" | "EXPECTED" => Some(Self::Pending),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_moves_from_draft_to_merged_or_closed() {
        let table = [
            (("OPEN", true, None), Some(PullState::Draft)),
            (("OPEN", true, Some("APPROVED")), Some(PullState::Draft)),
            (("OPEN", false, None), Some(PullState::Open)),
            (
                ("OPEN", false, Some("REVIEW_REQUIRED")),
                Some(PullState::Open),
            ),
            (
                ("OPEN", false, Some("CHANGES_REQUESTED")),
                Some(PullState::ChangesRequested),
            ),
            (("OPEN", false, Some("APPROVED")), Some(PullState::Approved)),
            (("MERGED", false, Some("APPROVED")), Some(PullState::Merged)),
            (("CLOSED", true, None), Some(PullState::Closed)),
            (("LOCKED", false, None), None),
        ];
        for ((state, draft, review), want) in table {
            assert_eq!(
                PullState::of(state, draft, review),
                want,
                "{state} {draft} {review:?}"
            );
        }
    }

    #[test]
    fn the_check_rollup_reads_as_passing_failing_or_pending() {
        let table = [
            ("SUCCESS", Some(Checks::Passing)),
            ("FAILURE", Some(Checks::Failing)),
            ("ERROR", Some(Checks::Failing)),
            ("PENDING", Some(Checks::Pending)),
            ("EXPECTED", Some(Checks::Pending)),
            ("NEUTRAL", None),
        ];
        for (rollup, want) in table {
            assert_eq!(Checks::of(rollup), want, "{rollup}");
        }
    }
}
