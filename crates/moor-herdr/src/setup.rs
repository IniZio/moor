//! The herdr config this front needs the user to add. The plugin never writes the user's
//! herdr config, so `moor-herdr herdr-setup` prints it.

use crate::boxes::BOX_MARK;
use crate::herdr::SOURCE;
use crate::sync::ENV_TOKEN;

/// The plugin action that `prefix+c` runs in place of herdr's plain new tab.
pub const NEW_ACTION: &str = "new";

/// The plugin action that `prefix+m` runs to open the Boxes panel.
pub const PANEL_ACTION: &str = "panel";

/// The config snippet: sidebar rows that set Boxes apart, and the key rebind.
#[must_use]
pub fn snippet() -> String {
    let (env, tag) = ENV_TOKEN;
    format!(
        r##"# moor: set Boxes apart from local worktrees in the sidebar.
[ui.sidebar.spaces]
rows = [
  ["state_icon", {{ token = "workspace", rules = [{{ starts_with = "{BOX_MARK}", fg = "#c678dd", bold = true }}] }}, {{ token = "$state", fg = "#e5c07b", bold = true }}],
  [{{ token = "${env}", dim = true, rules = [{{ equals = "{tag}", fg = "#c678dd", bold = true, dim = false }}] }}, "branch", "git_status"],
]

# moor: prefix+c runs moor's new in place of a plain tab. Merge into an existing [keys].
[keys]
new_tab = ""

[[keys.command]]
key = "prefix+c"
type = "plugin_action"
command = "{SOURCE}.{NEW_ACTION}"
description = "moor: new"

[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "{SOURCE}.{PANEL_ACTION}"
description = "moor: Boxes"
"##
    )
}

#[cfg(test)]
mod tests;
