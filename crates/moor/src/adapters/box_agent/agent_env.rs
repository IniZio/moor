//! The environment the Box agent starts with: crabbox's own names are cleared and the agent's own set.

use moor_core::boxscript::quote;

/// The variable that makes Claude Code accept bypassPermissions as root.
pub(super) const ROOT_SANDBOX_VAR: &str = "IS_SANDBOX";

/// The crabbox gate variables; the agent never sees them.
const GATE_VARS: [&str; 8] = [
    "child",
    "child_identity",
    "child_pid",
    "command_umask",
    "gate",
    "run_dir",
    "state",
    "token",
];

/// Shell lines that unset the crabbox gate names, every `NSC_*` name and `token` in any case.
pub(super) fn clear_lines() -> Vec<String> {
    vec![
        format!("unset {}", GATE_VARS.join(" ")),
        "for moor_v in $(env | sed -n 's/^\\(NSC_[A-Za-z0-9_]*\\)=.*/\\1/p; \
         s/^\\([Tt][Oo][Kk][Ee][Nn]\\)=.*/\\1/p'); do unset \"$moor_v\"; done; unset moor_v"
            .to_owned(),
    ]
}

/// True when `name` is a shell identifier.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `export` lines for `vars` whose name is not in `strip_env`, then `export IS_SANDBOX=1` when `root`
/// (last, so a repo value cannot replace it). Errors when a name is not a shell identifier.
pub(super) fn export_lines(
    vars: &[(String, String)],
    root: bool,
    strip_env: &[String],
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for (name, value) in vars {
        if !identifier(name) {
            return Err(format!("`{name}` is not a valid environment variable name"));
        }
        if name == ROOT_SANDBOX_VAR || strip_env.contains(name) {
            continue;
        }
        lines.push(format!("export {name}={}", quote(value)));
    }
    if root {
        lines.push(format!("export {ROOT_SANDBOX_VAR}=1"));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests;
