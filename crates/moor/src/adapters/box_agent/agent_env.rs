//! The environment the Box agent starts with.

use std::collections::BTreeMap;

use moor_core::boxscript::quote;
use moor_core::name::BoxPath;

/// The variable that makes Claude Code accept bypassPermissions as root.
pub(super) const ROOT_SANDBOX_VAR: &str = "IS_SANDBOX";

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

/// `export` lines for a profile's `env`, each value quoted so it stays literal, with `{home}` and
/// `{profile}` replaced by the Box user's `home` and the profile checkout `profile`. A name in
/// `strip_env` is skipped, as in [`export_lines`].
#[expect(
    clippy::literal_string_with_formatting_args,
    reason = "{home} and {profile} are the profile's placeholders, not format arguments"
)]
pub(super) fn profile_lines(
    env: &BTreeMap<String, String>,
    (home, profile): (&BoxPath, &BoxPath),
    strip_env: &[String],
) -> Result<Vec<String>, String> {
    let vars: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| {
            let value = value
                .replace("{home}", home.as_str())
                .replace("{profile}", profile.as_str());
            (name.clone(), value)
        })
        .collect();
    export_lines(&vars, false, strip_env)
}

#[cfg(test)]
mod tests;
