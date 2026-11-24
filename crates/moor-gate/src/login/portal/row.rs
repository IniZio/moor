//! A Box as a list row or the head of a Box page: what `show` sent of it, and the JSON a page reads.

use moor_core::progress::ProgressView;
use moor_core::view::BoxTies;
use moor_core::workflow::{is_draft_prompt, split_plan_first};
use serde::Deserialize;
use serde_json::{json, Value};

use super::pr_json;

/// A Box row as `show` sends it, the parts a page reads.
#[derive(Deserialize)]
pub(super) struct Row {
    /// The Box name.
    pub(super) name: String,
    /// The repo, as `OWNER/NAME`.
    pub(super) repo: Option<String>,
    /// The repos cloned beside `repo`, in order.
    #[serde(default)]
    pub(super) other_repos: Vec<String>,
    /// The branch.
    pub(super) branch: Option<String>,
    /// The base ref.
    pub(super) base: Option<String>,
    /// The issue the Box was started for, as the person wrote it.
    pub(super) issue: Option<String>,
    /// What the Box was asked.
    pub(super) prompt: Option<String>,
    /// RFC 3339 UTC last update.
    pub(super) updated_at: String,
    /// The lifecycle, as `show` names it.
    pub(super) lifecycle: Option<String>,
    /// The derived state.
    pub(super) progress: Option<ProgressView>,
    /// The Box's gates and where it stands, handed on as `show` sent it.
    pub(super) gate: Option<Value>,
    /// The workflow run behind the Box and the node it runs on; none sent by a controller that predates them.
    #[serde(default)]
    pub(super) ties: BoxTies,
    /// Who the Box acts for, when it is not the person reading it.
    #[serde(default)]
    pub(super) owner: Option<String>,
    /// When the task was asked, for a task with no work Box behind it.
    #[serde(default)]
    pub(super) task_at: Option<String>,
}

impl Row {
    /// Whether the Box is a row of Tasks home: not a draft, and not a work Box a place started,
    /// which shows in its task instead.
    pub(super) fn on_home(&self) -> bool {
        !self.prompt.as_deref().is_some_and(is_draft_prompt) && self.ties.task.is_none()
    }
}

/// A Box as a list row or the head of a Box page.
pub(super) fn summary(row: &Row) -> Option<Value> {
    let progress = row.progress.as_ref()?;
    Some(json!({
        "box": row.name,
        "repo": row.repo,
        "other_repos": row.other_repos,
        "branch": row.branch,
        "base": row.base,
        "issue": row.issue,
        "machine": row.ties.machine,
        "workflow": row.ties.workflow,
        "prompt": row.prompt.as_deref().map(|p| split_plan_first(p).unwrap_or(p)),
        "plan_first": row.prompt.as_deref().and_then(split_plan_first).is_some(),
        "title": progress.title,
        "state": progress.state,
        "failed": progress.failed,
        "stale": progress.stale,
        "line": progress.line,
        "last_event_at": progress.last_event_at,
        "updated_at": row.updated_at,
        "pr": pr_json(progress.pr.as_ref()),
        "needs": progress.needs,
        "gate": row.gate,
        "retryable": row.lifecycle.as_deref() == Some("setup_failed"),
        "owner": row.owner,
        "task_at": row.task_at,
    }))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test assertions index the JSON")]
mod tests {
    use super::*;

    /// A Box row as `show` sends it, with `extra` set over it.
    fn shown(extra: &Value) -> Value {
        let mut row = json!({
            "name": "web/mcp-1a2b3c4d", "repo": "acme/web", "issue": null, "prompt": "Fix the typo",
            "lifecycle": "ready", "updated_at": "2026-11-18T10:00:00Z",
            "progress": {"title": "Fix the typo", "state": "working", "stale": false,
                "line": "Editing", "last_event_at": null, "pr": null}});
        for (key, value) in extra.as_object().into_iter().flatten() {
            row[key] = value.clone();
        }
        summary(&serde_json::from_value(row).unwrap()).unwrap()
    }

    /// A Box row as `show` sends it, with `extra` set over it, before it is summarised.
    fn row_with(extra: &Value) -> Row {
        let mut row = json!({
            "name": "web/mcp-1a2b3c4d", "repo": "acme/web", "issue": null, "prompt": "Fix the typo",
            "lifecycle": "ready", "updated_at": "2026-11-18T10:00:00Z", "progress": null});
        for (key, value) in extra.as_object().into_iter().flatten() {
            row[key] = value.clone();
        }
        serde_json::from_value(row).unwrap()
    }

    #[test]
    fn portal_list_drops_a_row_tied_to_a_task() {
        assert!(row_with(&json!({})).on_home());
        assert!(row_with(&json!({"ties": {"machine": "big"}})).on_home());
        assert!(!row_with(&json!({"ties": {"task": "p"}})).on_home());
    }

    #[test]
    fn another_users_box_names_its_owner_and_the_persons_own_names_none() {
        assert_eq!(
            shown(&json!({"owner": "ana@example.com"}))["owner"],
            "ana@example.com"
        );
        assert_eq!(shown(&json!({}))["owner"], Value::Null);
    }

    #[test]
    fn a_task_a_question_started_carries_when_it_was_asked() {
        let asked = shown(&json!({"task_at": "2026-11-24T10:58:00Z"}));
        assert_eq!(asked["task_at"], "2026-11-24T10:58:00Z");
        assert_eq!(shown(&json!({}))["task_at"], Value::Null);
    }

    #[test]
    fn a_box_with_an_issue_carries_it() {
        let row = shown(&json!({"issue": "ACME-12"}));
        assert_eq!(row["issue"], "ACME-12");
        assert_eq!(row["workflow"], Value::Null);
    }

    #[test]
    fn a_box_a_workflow_run_for_a_pull_request_started_names_the_workflow_and_the_pull_request() {
        let ties = json!({"workflow": {"name": "review", "pull": "acme/web#65"}, "machine": "big"});
        let row = shown(&json!({"ties": ties}));
        assert_eq!(
            row["workflow"],
            json!({"name": "review", "pull": "acme/web#65"})
        );
        assert_eq!(row["machine"], "big");
    }

    #[test]
    fn a_plain_box_and_a_box_from_an_older_controller_have_none_of_them() {
        let row = shown(&json!({}));
        for key in ["issue", "machine", "workflow"] {
            assert_eq!(row[key], Value::Null, "{key}");
        }
        let cron = shown(&json!({"ties": {"workflow": {"name": "nightly"}}}));
        assert_eq!(
            cron["workflow"],
            json!({"name": "nightly"}),
            "no pull request, no pull key"
        );
    }
}
