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
