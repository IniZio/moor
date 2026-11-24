//! What a Box is born as: a work Box, or a place the agent waits in until a task claims it.

use crate::name::ForgeRepo;

/// What a Box was started as. A start fixes it for the Box's life.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BoxKind {
    /// A Box a person's task works in.
    #[default]
    Work,
    /// A repo-less Box the agent waits in, started before any task and claimed by one.
    Place(PlaceState),
}

/// Where a place stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceState {
    /// Waiting for a task; no list, stream, sleep or close sees it.
    Idle,
    /// Taken by a task, which makes it an ordinary task from then on.
    Claimed(Claim),
}

/// The task that took a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The task's repo, the first one named.
    pub repo: ForgeRepo,
    /// The repos named after it, in order.
    pub also: Vec<ForgeRepo>,
    /// The person's words as typed.
    pub prompt: String,
    /// When the task took the place, in milliseconds since the epoch.
    pub at: i64,
    /// When the claim's rules were stored and the Box's egress had its hosts, in milliseconds
    /// since the epoch; none before.
    pub served: Option<i64>,
    /// When the first prompt left the inbox, in milliseconds since the epoch; none before.
    pub steered: Option<i64>,
}

impl BoxKind {
    /// Whether this is a place no task has claimed. The one check every reader that hides idle
    /// places asks.
    #[must_use]
    pub const fn is_idle_place(&self) -> bool {
        matches!(self, Self::Place(PlaceState::Idle))
    }

    /// The word the store keeps: `work` or `place`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Place(_) => "place",
        }
    }
}

impl Claim {
    /// The claim's repos, the task's first.
    pub fn repos(&self) -> impl Iterator<Item = &ForgeRepo> {
        std::iter::once(&self.repo).chain(&self.also)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> Claim {
        Claim {
            repo: ForgeRepo::new("o", "a"),
            also: vec![ForgeRepo::new("o", "b")],
            prompt: "where".to_owned(),
            at: 5,
            served: None,
            steered: None,
        }
    }

    #[test]
    fn only_an_unclaimed_place_is_idle() {
        assert!(BoxKind::Place(PlaceState::Idle).is_idle_place());
        assert!(!BoxKind::Work.is_idle_place());
        assert!(!BoxKind::Place(PlaceState::Claimed(claim())).is_idle_place());
    }

    #[test]
    fn a_claim_lists_its_task_repo_first() {
        let repos: Vec<String> = claim().repos().map(ToString::to_string).collect();
        assert_eq!(repos, ["o/a", "o/b"]);
    }
}
