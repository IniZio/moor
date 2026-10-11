use crate::fake::Rule;
use crate::world::World;

const WORKTREE_CREATE: &str = concat!(
    "git -C '{flag:--cwd}' worktree add -q -b '{flag:--branch}' '{root}/trees/wt-{n}' >/dev/null 2>&1 && ",
    "printf '%s' '{\"id\":\"cli:worktree:create\",\"result\":{\"type\":\"worktree_created\",\"workspace\":{\"workspace_id\":\"ws-{n}\"},",
    "\"worktree\":{\"path\":\"{root}/trees/wt-{n}\",\"branch\":\"{flag:--branch}\",",
    "\"open_workspace_id\":\"ws-{n}\"}}}'"
);

const WORKSPACE_CREATED: &str = r#"{"id":"cli:workspace:create","result":{"type":"workspace_created","workspace":{"workspace_id":"ws-{n}"},"tab":{"tab_id":"t-{n}"},"root_pane":{"pane_id":"p-{n}"}}}"#;

pub fn install(w: &World) {
    w.add_rules(
        "herdr",
        [
            Rule::on(&["worktree", "list"]).stdout(
                r#"{"id":"cli:worktree:list","result":{"type":"worktree_list","worktrees":[]}}"#,
            ),
            Rule::on(&["worktree", "create"]).exec(WORKTREE_CREATE),
            Rule::on(&["workspace", "create"]).stdout(WORKSPACE_CREATED),
            Rule::on(&["pane", "list"]).stdout(
                r#"{"id":"cli:pane:list","result":{"type":"pane_list","panes":[{"pane_id":"p-1","agent_status":"unknown"}]}}"#,
            ),
            Rule::on(&["tab", "list"]).stdout(
                r#"{"id":"cli:tab:list","result":{"type":"tab_list","tabs":[]}}"#,
            ),
            Rule::on(&["tab", "create"]).stdout(
                r#"{"id":"cli:tab:create","result":{"type":"tab_created","root_pane":{"pane_id":"p-{n}"},"tab":{"tab_id":"t-{n}"}}}"#,
            ),
        ],
    );
}

pub fn worktrees(w: &World, entries: &[(&str, &str, Option<&str>)]) {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|(branch, path, ws)| {
            serde_json::json!({ "branch": branch, "path": path, "open_workspace_id": ws })
        })
        .collect();
    let body = serde_json::json!({ "id": "cli:worktree:list", "result": { "type": "worktree_list", "worktrees": items } }).to_string();
    w.prepend_rules("herdr", [Rule::on(&["worktree", "list"]).stdout(body)]);
}

pub fn workspace_create(w: &World, id: &str) {
    let body = format!(
        r#"{{"id":"cli:workspace:create","result":{{"type":"workspace_created","workspace":{{"workspace_id":"{id}"}},"tab":{{"tab_id":"{id}:t1"}},"root_pane":{{"pane_id":"{id}:p1"}}}}}}"#
    );
    w.prepend_rules("herdr", [Rule::on(&["workspace", "create"]).stdout(body)]);
}

pub fn calls_of(w: &World, a: &str, b: &str) -> Vec<crate::fake::Call> {
    w.calls("herdr")
        .into_iter()
        .filter(|c| {
            c.argv.first().map(String::as_str) == Some(a)
                && c.argv.get(1).map(String::as_str) == Some(b)
        })
        .collect()
}

pub fn pane_list(w: &World, status: Option<&str>) {
    let pane = match status {
        Some(s) => format!(r#"{{"pane_id":"p-1","agent_status":"{s}"}}"#),
        None => r#"{"pane_id":"p-1"}"#.to_string(),
    };
    let body =
        format!(r#"{{"id":"cli:pane:list","result":{{"type":"pane_list","panes":[{pane}]}}}}"#);
    w.prepend_rules("herdr", [Rule::on(&["pane", "list"]).stdout(body)]);
}

pub fn pane_gone(w: &World) {
    w.prepend_rules(
        "herdr",
        [Rule::on(&["pane", "list"])
            .stdout(r#"{"id":"cli:pane:list","result":{"type":"pane_list","panes":[]}}"#)],
    );
}

pub fn pane_text(w: &World, text: &str) {
    w.prepend_rules("herdr", [Rule::on(&["pane", "read"]).stdout(text)]);
}

/// Real herdr error shape: `{"error":{"code","message"}}`, exit 1. `times` limits it to the first N calls.
pub fn error(w: &World, argv: &[&str], code: &str, message: &str, times: Option<u32>) {
    let body = serde_json::json!({ "error": { "code": code, "message": message } }).to_string();
    let mut r = Rule::on(argv).stdout(body).exit(1);
    if let Some(n) = times {
        r = r.times(n);
    }
    w.prepend_rules("herdr", [r]);
}

/// Every herdr call fails with `server_not_running` (first `times` calls if given).
pub fn server_not_running(w: &World, times: Option<u32>) {
    let body = serde_json::json!({ "error": { "code": "server_not_running", "message": "herdr server is not running" } }).to_string();
    let mut r = Rule::any().stdout(body).exit(1);
    if let Some(n) = times {
        r = r.times(n);
    }
    w.prepend_rules("herdr", [r]);
}

/// `invalid_request` "unknown variant" for an action this herdr does not support.
pub fn unknown_variant(w: &World, argv: &[&str], variant: &str) {
    error(
        w,
        argv,
        "invalid_request",
        &format!("unknown variant `{variant}`"),
        None,
    );
}
