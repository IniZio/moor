//! Reading the pull request whose head is a branch.

use std::time::Duration;

use moor_core::seam::forge::{BranchPull, Checks, ForgeError, ForgeRepo, Pull, PullState};
use moor_core::Secret;
use serde::Deserialize;

use super::{bearer, post, unavailable_errors, unexpected_shape, GqlError};

/// The longest a pull request read may take; a page waits on it.
const PULL_TIMEOUT: Duration = Duration::from_secs(10);

/// Asks whether the branch exists, and for the newest pull requests whose head ref is the branch,
/// with each one's head repo, review decision and the check rollup of its newest commit.
const PULL_QUERY: &str = r"query($owner: String!, $name: String!, $head: String!, $ref: String!) {
  repository(owner: $owner, name: $name) {
    ref(qualifiedName: $ref) { name }
    pullRequests(headRefName: $head, first: 10, orderBy: {field: CREATED_AT, direction: DESC}) {
      nodes {
        number state isDraft reviewDecision
        headRepository { nameWithOwner }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}";

/// Reads the pull request from `branch` of `repo` from the GraphQL endpoint `api` as the
/// account of `token`.
pub(in crate::adapters::github) fn pull_of(
    api: &str,
    timeout: Duration,
    repo: &ForgeRepo,
    branch: &str,
    token: &Secret,
) -> Result<BranchPull, ForgeError> {
    let auth = bearer(token)?;
    let body = serde_json::json!({
        "query": PULL_QUERY,
        "variables": {
            "owner": repo.owner, "name": repo.name, "head": branch,
            "ref": format!("refs/heads/{branch}"),
        },
    });
    let (status, reply) = post(api, timeout.min(PULL_TIMEOUT), &auth, &body.to_string())?;
    match status {
        200 => pick(
            serde_json::from_slice(&reply).map_err(|_| unexpected_shape())?,
            repo,
        ),
        401 => Err(ForgeError::Rejected),
        s => Err(ForgeError::Unavailable(format!("GitHub answered HTTP {s}"))),
    }
}

/// A GraphQL reply to [`PULL_QUERY`].
#[derive(Deserialize)]
struct Reply {
    /// The answer; parts the token may not read are null.
    data: Option<Data>,
    /// Errors, each with a GitHub error type.
    #[serde(default)]
    errors: Vec<GqlError>,
}

/// The query's top-level field.
#[derive(Deserialize)]
struct Data {
    /// The repo, absent when it is not visible.
    repository: Option<Repository>,
}

/// The repo's branch and pull requests.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    /// The branch, absent when it was not pushed.
    #[serde(rename = "ref")]
    branch: Option<Named>,
    /// Its pull requests from the branch, newest first.
    pull_requests: Option<Nodes<Node>>,
}

/// A GraphQL connection's nodes; a node the token may not read is null.
#[derive(Deserialize)]
struct Nodes<T> {
    /// The nodes.
    nodes: Option<Vec<Option<T>>>,
}

/// One pull request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    /// Its number.
    number: u64,
    /// `OPEN`, `CLOSED` or `MERGED`.
    state: String,
    /// Whether it is a draft.
    is_draft: bool,
    /// `APPROVED`, `CHANGES_REQUESTED` or `REVIEW_REQUIRED`.
    review_decision: Option<String>,
    /// The repo its head branch is in, absent once that repo is gone.
    head_repository: Option<HeadRepo>,
    /// Its newest commit.
    commits: Option<Nodes<CommitNode>>,
}

/// A git ref, by name.
#[derive(Deserialize)]
struct Named {
    /// Its short name.
    #[serde(rename = "name")]
    _name: String,
}

/// A head repo.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadRepo {
    /// `OWNER/NAME`.
    name_with_owner: String,
}

/// A pull request's commit.
#[derive(Deserialize)]
struct CommitNode {
    /// The commit.
    commit: Option<Commit>,
}

/// A commit's checks.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    /// All its checks and statuses together; absent when it has none.
    status_check_rollup: Option<Rollup>,
}

/// A check rollup.
#[derive(Deserialize)]
struct Rollup {
    /// `SUCCESS`, `FAILURE`, `ERROR`, `PENDING` or `EXPECTED`.
    state: String,
}

/// Whether the branch is pushed, and the newest open pull request from `repo` itself in `reply`,
/// else the newest one. A part the token may not read, such as the checks, reads as unknown.
fn pick(reply: Reply, repo: &ForgeRepo) -> Result<BranchPull, ForgeError> {
    let Some(found) = reply.data.and_then(|d| d.repository) else {
        let not_found = |e: &GqlError| e.kind.as_deref() == Some("NOT_FOUND");
        return Err(if reply.errors.iter().all(not_found) {
            ForgeError::NotVisible
        } else {
            unavailable_errors(&reply.errors)
        });
    };
    let own = repo.to_string();
    let pushed = found.branch.is_some();
    let pulls: Vec<Pull> = found
        .pull_requests
        .and_then(|p| p.nodes)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|n| {
            n.head_repository
                .as_ref()
                .is_some_and(|h| h.name_with_owner.eq_ignore_ascii_case(&own))
        })
        .filter_map(pull)
        .collect();
    let pull = pulls
        .iter()
        .find(|p| p.state.is_open())
        .or_else(|| pulls.first())
        .cloned();
    Ok(BranchPull { pushed, pull })
}

/// The pull request `node` describes; none for a state GitHub does not name.
fn pull(node: Node) -> Option<Pull> {
    let state = PullState::of(&node.state, node.is_draft, node.review_decision.as_deref())?;
    let checks = node
        .commits
        .and_then(|c| c.nodes)
        .and_then(|n| n.into_iter().flatten().next())
        .and_then(|c| c.commit)
        .and_then(|c| c.status_check_rollup)
        .and_then(|r| Checks::of(&r.state))
        .filter(|_| state.is_open());
    Some(Pull {
        number: node.number,
        state,
        checks,
    })
}
