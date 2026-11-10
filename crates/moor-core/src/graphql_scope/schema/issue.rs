//! Contexts for an issue of the Box repo read by number, and the issues it links to.

use super::Next::{Conn as N, Ctx as C};
use super::Shape;

/// Issue contexts.
pub(super) const SHAPES: &[Shape] = &[
    Shape {
        name: "boxissue",
        leaves: &[
            "id",
            "number",
            "url",
            "state",
            "stateReason",
            "createdAt",
            "title",
            "body",
        ],
        links: &[
            ("author", C("actor")),
            ("milestone", C("milestone")),
            ("assignees", N("actor")),
            ("labels", N("label")),
            ("reactionGroups", C("reaction")),
            ("comments", N("comment")),
            ("issueType", C("issuetype")),
            ("parent", C("linkedissue")),
            ("subIssues", N("linkedissue")),
            ("subIssuesSummary", C("subissues")),
            ("blockedBy", N("linkedissue")),
            ("blocking", N("linkedissue")),
            ("projectItems", N("projitem")),
        ],
    },
    // A linked issue may be in another repo, so only its identity and headline are readable.
    Shape {
        name: "linkedissue",
        leaves: &["id", "number", "title", "url", "state"],
        links: &[("repository", C("reponame"))],
    },
    Shape {
        name: "reponame",
        leaves: &["nameWithOwner"],
        links: &[],
    },
    Shape {
        name: "issuetype",
        leaves: &["id", "name", "description", "color"],
        links: &[],
    },
    Shape {
        name: "subissues",
        leaves: &["total", "completed", "percentCompleted"],
        links: &[],
    },
];
