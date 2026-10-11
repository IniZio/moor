use crate::fake::Rule;
use crate::world::World;

const WORKTREE_CREATE: &str = concat!(
    "git -C '{flag:--cwd}' worktree add -q -b '{flag:--branch}' '{root}/trees/wt-{n}' >/dev/null 2>&1 && ",
    "printf '%s' '{\"result\":{\"workspace\":{\"workspace_id\":\"ws-{n}\"},",
    "\"worktree\":{\"path\":\"{root}/trees/wt-{n}\",\"branch\":\"{flag:--branch}\",",
    "\"open_workspace_id\":\"ws-{n}\"}}}'"
);

pub fn install(w: &World) {
    w.add_rules(
        "herdr",
        [
            Rule::on(&["worktree", "list"]).stdout(r#"{"result":{"worktrees":[]}}"#),
            Rule::on(&["worktree", "create"]).exec(WORKTREE_CREATE),
            Rule::on(&["pane", "list"]).stdout(r#"{"result":{"panes":[{"pane_id":"p-1"}]}}"#),
            Rule::on(&["tab", "list"]).stdout(r#"{"result":{"tabs":[]}}"#),
            Rule::on(&["tab", "create"]).stdout(r#"{"result":{"root_pane":{"pane_id":"p-{n}"}}}"#),
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
    let body = serde_json::json!({ "result": { "worktrees": items } }).to_string();
    w.prepend_rules("herdr", [Rule::on(&["worktree", "list"]).stdout(body)]);
}

pub fn workspace_create(w: &World, id: &str) {
    let body = format!(r#"{{"result":{{"workspace":{{"workspace_id":"{id}"}}}}}}"#);
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
    let body = format!(r#"{{"result":{{"panes":[{pane}]}}}}"#);
    w.prepend_rules("herdr", [Rule::on(&["pane", "list"]).stdout(body)]);
}

pub fn pane_gone(w: &World) {
    w.prepend_rules(
        "herdr",
        [Rule::on(&["pane", "list"]).stdout(r#"{"result":{"panes":[]}}"#)],
    );
}

pub fn pane_text(w: &World, text: &str) {
    w.prepend_rules("herdr", [Rule::on(&["pane", "read"]).stdout(text)]);
}
