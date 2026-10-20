//! Allow-list of the fields a request may select under an allowed root field, keyed by the
//! context (the kind of object) the field is selected on.
//!
//! A field name alone cannot tell `Repository.issues` from `User.issues`, so every context names
//! exactly which scalar fields and which links to other contexts it admits. Objects that can lead
//! to other repos or to account data (`User`, `Organization`) appear only as identity contexts.

/// Where a link field leads.
#[derive(Debug, Clone, Copy)]
pub(super) enum Next {
    /// A single object of the named context.
    Ctx(&'static str),
    /// A connection or list of objects of the named context.
    Conn(&'static str),
}

use Next::{Conn as N, Ctx as C};

mod pr;

/// One context: its scalar fields and its links.
pub(super) struct Shape {
    /// Context name.
    pub(super) name: &'static str,
    /// Fields that take no sub-selection.
    pub(super) leaves: &'static [&'static str],
    /// Fields that take a sub-selection, with the context it is read in.
    pub(super) links: &'static [(&'static str, Next)],
}

/// Returns the context with the given name.
pub(super) fn shape(name: &str) -> Option<&'static Shape> {
    all().find(|s| s.name == name)
}

/// Returns every context.
pub(super) fn all() -> impl Iterator<Item = &'static Shape> {
    SHAPES.iter().chain(pr::SHAPES)
}

/// Fields admitted inside a connection's `pageInfo`.
pub(super) const PAGE_INFO: &[&str] =
    &["hasNextPage", "hasPreviousPage", "startCursor", "endCursor"];

/// Context of the Box repo and of objects found by ID.
pub(super) const REPO: &str = "repo";
/// Context of a mutation payload.
pub(super) const PAYLOAD: &str = "payload";
/// All contexts.
pub(super) const SHAPES: &[Shape] = &[
    Shape {
        name: PAYLOAD,
        leaves: &["clientMutationId"],
        links: &[("pullRequest", C("pr")), ("assignable", C("pr"))],
    },
    Shape {
        name: REPO,
        leaves: &[
            "id",
            "databaseId",
            "name",
            "nameWithOwner",
            "url",
            "description",
            "isPrivate",
            "isFork",
            "isArchived",
            "isEmpty",
            "isTemplate",
            "visibility",
            "viewerPermission",
            "viewerCanAdminister",
            "viewerDefaultMergeMethod",
            "viewerDefaultCommitEmail",
            "hasIssuesEnabled",
            "hasProjectsEnabled",
            "hasWikiEnabled",
            "hasDiscussionsEnabled",
            "mergeCommitAllowed",
            "squashMergeAllowed",
            "rebaseMergeAllowed",
            "autoMergeAllowed",
            "deleteBranchOnMerge",
            "pushedAt",
            "createdAt",
            "updatedAt",
            "homepageUrl",
            "sshUrl",
            "stargazerCount",
            "forkCount",
        ],
        links: &[
            ("owner", C("actor")),
            ("parent", C("parent")),
            ("defaultBranchRef", C("ref")),
            ("ref", C("ref")),
            ("refs", N("ref")),
            ("pullRequest", C("pr")),
            ("pullRequests", N("pr")),
            ("labels", N("label")),
            ("milestones", N("milestone")),
            ("assignableUsers", N("actor")),
            ("suggestedActors", N("actor")),
            ("projectsV2", N("project")),
            ("pullRequestTemplates", C("template")),
            ("issueTemplates", C("template")),
            ("mergeQueue", C("mqentry")),
            ("issues", N("listissue")),
        ],
    },
    Shape {
        name: "repoid",
        leaves: &[
            "id",
            "databaseId",
            "name",
            "nameWithOwner",
            "url",
            "isPrivate",
            "isFork",
            "viewerPermission",
        ],
        links: &[("owner", C("actor"))],
    },
    Shape {
        name: "parent",
        leaves: &[
            "id",
            "databaseId",
            "name",
            "nameWithOwner",
            "url",
            "sshUrl",
            "description",
            "isPrivate",
            "isFork",
            "hasIssuesEnabled",
            "hasWikiEnabled",
            "viewerPermission",
        ],
        links: &[("owner", C("actor")), ("defaultBranchRef", C("refname"))],
    },
    Shape {
        name: "refname",
        leaves: &["name"],
        links: &[],
    },
    Shape {
        name: "actor",
        leaves: &[
            "id",
            "login",
            "name",
            "slug",
            "combinedSlug",
            "url",
            "avatarUrl",
            "databaseId",
        ],
        links: &[("organization", C("actor"))],
    },
    Shape {
        name: "ref",
        leaves: &["id", "name", "prefix"],
        links: &[
            ("target", C("commit")),
            ("associatedPullRequests", N("prref")),
            ("rules", N("rule")),
            ("branchProtectionRule", C("protection")),
        ],
    },
    Shape {
        name: "commit",
        leaves: &[
            "id",
            "oid",
            "abbreviatedOid",
            "message",
            "messageHeadline",
            "messageBody",
            "committedDate",
            "authoredDate",
            "url",
        ],
        links: &[
            ("statusCheckRollup", C("rollup")),
            ("authors", N("gitactor")),
        ],
    },
    Shape {
        name: "gitactor",
        leaves: &["id", "name", "email"],
        links: &[("user", C("actor"))],
    },
    Shape {
        name: "rollup",
        leaves: &["id", "state"],
        links: &[("contexts", C("ctxconn"))],
    },
    Shape {
        name: "ctxconn",
        leaves: &["totalCount", "checkRunCount", "statusContextCount"],
        links: &[
            ("nodes", C("checkctx")),
            ("edges", C("ctxedge")),
            ("pageInfo", C("pageinfo")),
            ("checkRunCountsByState", C("statecount")),
            ("statusContextCountsByState", C("statecount")),
        ],
    },
    Shape {
        name: "ctxedge",
        leaves: &["cursor"],
        links: &[("node", C("checkctx"))],
    },
    Shape {
        name: "pageinfo",
        leaves: &["hasNextPage", "hasPreviousPage", "startCursor", "endCursor"],
        links: &[],
    },
    Shape {
        name: "checkctx",
        leaves: &[
            "id",
            "context",
            "state",
            "targetUrl",
            "description",
            "createdAt",
            "isRequired",
            "name",
            "status",
            "conclusion",
            "detailsUrl",
            "startedAt",
            "completedAt",
        ],
        links: &[("checkSuite", C("suite"))],
    },
    Shape {
        name: "suite",
        leaves: &["id"],
        links: &[("workflowRun", C("run"))],
    },
    Shape {
        name: "run",
        leaves: &["id", "event"],
        links: &[("workflow", C("workflow"))],
    },
    Shape {
        name: "workflow",
        leaves: &["id", "name"],
        links: &[],
    },
    Shape {
        name: "prref",
        leaves: &[
            "id",
            "number",
            "url",
            "state",
            "isDraft",
            "baseRefName",
            "headRefName",
            "isCrossRepository",
        ],
        links: &[
            ("baseRepository", C("repoid")),
            ("headRepository", C("repoid")),
        ],
    },
    Shape {
        name: "reaction",
        leaves: &["content"],
        links: &[("users", C("count"))],
    },
    Shape {
        name: "count",
        leaves: &["totalCount"],
        links: &[],
    },
    Shape {
        name: "fieldval",
        leaves: &["optionId", "name"],
        links: &[],
    },
    Shape {
        name: "mqentry",
        leaves: &["id"],
        links: &[],
    },
    Shape {
        name: "statecount",
        leaves: &["state", "count"],
        links: &[],
    },
    Shape {
        name: "protection",
        leaves: &["requiresStrictStatusChecks", "requiredApprovingReviewCount"],
        links: &[],
    },
    Shape {
        name: "rule",
        leaves: &["type"],
        links: &[],
    },
];
