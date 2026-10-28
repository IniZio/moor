//! The herdr config this front needs the user to add. The plugin never writes the user's
//! herdr config, so `moor-herdr herdr-setup` prints it.

use crate::boxes::BOX_MARK;
use crate::herdr::SOURCE;
use crate::shell::HOST_SHELL_ENV;
use crate::sync::ENV_TOKEN;

/// The plugin action that `prefix+c` runs in place of herdr's plain new tab.
pub const NEW_ACTION: &str = "new";

/// The plugin action that `prefix+m` runs to open the Boxes panel.
pub const PANEL_ACTION: &str = "panel";

/// The plugin's default-shell wrapper: under the linked plugin when herdr runs this, else in
/// the source tree this binary was built from.
#[must_use]
pub fn shell_path() -> String {
    std::env::var("HERDR_PLUGIN_ROOT").map_or_else(
        |_| concat!(env!("CARGO_MANIFEST_DIR"), "/herdr/bin/shell.sh").to_owned(),
        |root| format!("{root}/bin/shell.sh"),
    )
}

/// The config snippet: sidebar rows that set Boxes apart, the key rebind, and the default
/// shell at `shell` that opens new panes of a Box's workspace in the Box.
#[must_use]
pub fn snippet(shell: &str) -> String {
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

# moor: a new tab in a Box's workspace opens a shell in the Box; other tabs get your shell.
# Had you set default_shell, put it in the plugin env file as {HOST_SHELL_ENV}=<path>.
[terminal]
default_shell = "{shell}"
"##
    )
}

#[cfg(test)]
mod tests;
