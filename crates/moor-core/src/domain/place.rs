//! What a Box is born as: a work Box, or a place an agent works from for one task.

use crate::name::ForgeRepo;

/// What a Box was started as. A start fixes it for the Box's life.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BoxKind {
    /// A Box a person's task works in.
    #[default]
    Work,
    /// A repo-less Box an agent works from, started for a task.
    Place(PlaceState),
}

/// Where a place stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceState {
    /// Held by no task: a place started before every place came with its task, as the store
    /// still holds it. A list, sleep and close treat it as any other Box.
    Idle,
    /// Held by its task from its start.
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

    /// The first message the claimed place's agent reads: the repos, a blank line, then the
    /// person's words.
    #[must_use]
    pub fn first_message(&self) -> String {
        let repos: Vec<String> = self.repos().map(ToString::to_string).collect();
        format!("Repos: {}\n\n{}", repos.join(", "), self.prompt)
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
    fn the_first_message_names_the_repos_and_then_the_words() {
        assert_eq!(claim().first_message(), "Repos: o/a, o/b\n\nwhere");
    }

    #[test]
    fn a_claim_lists_its_task_repo_first() {
        let repos: Vec<String> = claim().repos().map(ToString::to_string).collect();
        assert_eq!(repos, ["o/a", "o/b"]);
    }
}
