//! Claude Code's install on a Box that lacks it: Anthropic's native installer.

use moor_core::boxscript::quote;
use moor_core::seam::agent_profile::AgentInstall;
use moor_core::seam::proc::Tool;

use super::super::launch::AGENT_PATH_LINE;

/// The installer Anthropic publishes; it puts the binary under `~/.local/share/claude` and links
/// it from `~/.local/bin`, and picks the version itself.
const INSTALLER_URL: &str = "https://claude.ai/install.sh";

/// The directory that holds the installed versions.
const INSTALL_DIR: &str = "~/.local/share/claude";

/// The script that runs the installer when the agent command is not on the Box's `PATH`.
///
/// The installer is downloaded to a private temp file first, so a failed download fails the step
/// instead of feeding a half-read script to a shell.
fn script() -> String {
    let program = quote(Tool::Agent.program());
    let url = quote(INSTALLER_URL);
    let missing = |what: &str| {
        quote(&format!(
            "moor: the Box has no {what}, so the agent cannot be installed"
        ))
    };
    [
        AGENT_PATH_LINE.to_owned(),
        format!("command -v {program} >/dev/null 2>&1 && exit 0"),
        format!(
            "command -v curl >/dev/null 2>&1 || {{ echo {} >&2; exit 1; }}",
            missing("curl")
        ),
        format!(
            "command -v bash >/dev/null 2>&1 || {{ echo {} >&2; exit 1; }}",
            missing("bash")
        ),
        "t=$(mktemp) || exit 1".to_owned(),
        "trap 'rm -f \"$t\"' EXIT".to_owned(),
        format!("curl -fsSL {url} -o \"$t\" && bash \"$t\" || exit 1"),
        format!("command -v {program} >/dev/null 2>&1"),
    ]
    .join("\n")
}

/// The install of the Claude Code agent.
pub(super) fn native() -> AgentInstall {
    AgentInstall {
        script: script(),
        dirs: vec![INSTALL_DIR.to_owned()],
    }
}

#[cfg(test)]
mod tests;
