//! Where in a Box a command runs.

use serde::{Deserialize, Serialize};

/// Where in a Box a command runs. Every Box has both. The workdir holds the same files at the
/// same path in both; each has its own home. There is no default: every caller says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    /// Where the agent's terminal and moor's agent plumbing run; it outlives any failure of
    /// [`Place::Work`].
    Agent,
    /// Where repos build and services run; the machine providing the Box brings it back when it
    /// fails.
    Work,
}

impl Place {
    /// The place's name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Work => "work",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Place;

    #[test]
    fn a_place_is_its_kebab_case_name_on_the_wire() {
        let work = serde_json::to_string(&Place::Work).unwrap_or_default();
        assert_eq!(work, "\"work\"");
        let agent: Option<Place> = serde_json::from_str("\"agent\"").ok();
        assert_eq!(agent, Some(Place::Agent));
        assert_eq!(Place::Agent.as_str(), "agent");
    }
}
